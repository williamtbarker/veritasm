//! Bounded byte-oriented FASTA/FASTQ parsing.
//!
//! Headers are intentionally not UTF-8 strings. Only normalized identity
//! digests leave this module; descriptions and raw identifiers are discarded
//! after synchronization.

use crate::config::Limits;
use crate::error::{overflow, ErrorCode, Result, VeritasmError};
use crate::input::{DecodedReader, PreparedSource};
use crate::model::{FastxFormat, MateRole};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::fs::File;
use std::io::{BufRead, BufReader, Read};

const NORMALIZED_ID_DOMAIN: &[u8] = b"veritasm:normalized-id:v1\0";
pub(crate) const READER_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRecord {
    /// Retained only until paired streams are synchronized.
    pub normalized_identity: Vec<u8>,
    pub normalized_identity_digest: [u8; 32],
    pub sequence: Vec<u8>,
    pub quality: Option<Vec<u8>>,
    pub inferred_role: bool,
}

#[derive(Debug)]
struct ParsedHeader {
    identity: Vec<u8>,
    role: Option<MateRole>,
}

/// Streaming parser over one already-decoded immutable source.
pub struct FastxReader {
    lines: BoundedLines<DecodedReader>,
    format: FastxFormat,
    expected_role: MateRole,
    label: String,
    limits: Limits,
    memory_cap: u64,
    next_record_ordinal: u64,
    records: u64,
    bases: u64,
    inferred_roles: u64,
    authenticated: bool,
}

impl FastxReader {
    pub fn open(source: &PreparedSource, limits: &Limits, memory_cap: u64) -> Result<Self> {
        if memory_cap == 0 {
            return Err(VeritasmError::new(
                ErrorCode::ResourceMemory,
                format!("{}: parser memory allowance is zero", source.spec.label()),
            ));
        }
        let file = source.open_decoded()?;
        let label = source.spec.label();
        let mut lines = BoundedLines::new(file, limits.max_record_bytes, label.clone());
        let format = loop {
            match lines.peek_byte()? {
                None => {
                    lines.authenticate_eof()?;
                    return Err(VeritasmError::new(
                        ErrorCode::InputEmpty,
                        format!("{label}: source contains zero records"),
                    ));
                }
                Some(b'>') => break FastxFormat::Fasta,
                Some(b'@') => break FastxFormat::Fastq,
                Some(_) => {
                    let line_memory = memory_cap;
                    let line = lines.read_line(
                        false,
                        memory_cap.min(limits.max_record_bytes),
                        ErrorCode::ResourceMemory,
                        line_memory,
                        "leading line",
                    )?;
                    if !trim_ascii(&line).is_empty() {
                        return Err(VeritasmError::new(
                            ErrorCode::InputFormat,
                            format!("{label}: expected a FASTA '>' or FASTQ '@' header"),
                        ));
                    }
                }
            }
        };
        Ok(Self {
            lines,
            format,
            expected_role: source.spec.role,
            label,
            limits: limits.clone(),
            memory_cap,
            next_record_ordinal: 0,
            records: 0,
            bases: 0,
            inferred_roles: 0,
            authenticated: false,
        })
    }

    pub const fn format(&self) -> FastxFormat {
        self.format
    }

    pub fn next_record(&mut self) -> Result<Option<ParsedRecord>> {
        if self.authenticated {
            return Ok(None);
        }
        let record = match self.format {
            FastxFormat::Fasta => self.next_fasta(),
            FastxFormat::Fastq => self.next_fastq(),
        }?;
        match record {
            Some(record) => {
                self.settle_record_boundary()?;
                Ok(Some(record))
            }
            None => {
                self.lines.authenticate_eof()?;
                self.authenticated = true;
                Ok(None)
            }
        }
    }

    /// Whether this reader reached and authenticated the registered decoded EOF.
    pub(crate) const fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    pub const fn records(&self) -> u64 {
        self.records
    }

    pub const fn bases(&self) -> u64 {
        self.bases
    }

    pub const fn inferred_roles(&self) -> u64 {
        self.inferred_roles
    }

    /// Consume legal trailing FASTQ blank lines and authenticate before the
    /// final record is released. FASTA parsing already consumes sequence-line
    /// blanks while looking ahead to the next header or EOF.
    fn settle_record_boundary(&mut self) -> Result<()> {
        if self.format == FastxFormat::Fastq {
            loop {
                match self.lines.peek_byte()? {
                    None | Some(b'@') => break,
                    Some(_) => {
                        let line_memory = self.line_memory_limit(0)?;
                        let line = self.lines.read_line(
                            false,
                            line_memory.min(self.limits.max_record_bytes),
                            ErrorCode::ResourceMemory,
                            line_memory,
                            "inter-record FASTQ line",
                        )?;
                        if !trim_ascii(&line).is_empty() {
                            return Err(self.error(
                                ErrorCode::InputFastqStructure,
                                "expected a FASTQ '@' header at a record boundary",
                            ));
                        }
                    }
                }
            }
        }
        if self.lines.peek_byte()?.is_none() {
            self.lines.authenticate_eof()?;
            self.authenticated = true;
        }
        Ok(())
    }

    fn next_fasta(&mut self) -> Result<Option<ParsedRecord>> {
        match self.lines.peek_byte()? {
            None => return Ok(None),
            Some(b'>') => {}
            Some(_) => {
                return Err(self.error(
                    ErrorCode::InputFastaStructure,
                    "expected a FASTA header at a record boundary",
                ));
            }
        }
        self.lines.begin_record()?;
        let (header_limit, header_limit_code) = self.header_line_limit()?;
        let header_line = self.lines.read_line(
            true,
            header_limit,
            header_limit_code,
            self.memory_cap,
            "FASTA header",
        )?;
        let header = self.parse_header_line(&header_line, b'>', 0, header_line.capacity())?;
        drop(header_line);
        let mut sequence = Vec::new();

        loop {
            match self.lines.peek_byte()? {
                None | Some(b'>') => break,
                Some(_) => {
                    let retained = checked_usize_sum(
                        header.identity.capacity(),
                        sequence.capacity(),
                        "FASTA retained parser bytes",
                    )?;
                    let line_memory = self.line_memory_limit(retained)?;
                    let line = self.lines.read_line(
                        true,
                        line_memory,
                        ErrorCode::ResourceMemory,
                        line_memory,
                        "FASTA sequence line",
                    )?;
                    let symbols = trim_ascii(&line);
                    if symbols.is_empty() {
                        continue;
                    }
                    self.extend_sequence(
                        &mut sequence,
                        symbols,
                        header.identity.capacity(),
                        line.capacity(),
                    )?;
                }
            }
        }
        self.lines.end_record();
        if sequence.is_empty() {
            return Err(self.error(
                ErrorCode::InputFastaStructure,
                "FASTA record has an empty sequence",
            ));
        }
        self.complete_record(header, sequence, None).map(Some)
    }

    fn next_fastq(&mut self) -> Result<Option<ParsedRecord>> {
        loop {
            match self.lines.peek_byte()? {
                None => return Ok(None),
                Some(b'@') => break,
                Some(_) => {
                    let line_memory = self.line_memory_limit(0)?;
                    let line = self.lines.read_line(
                        false,
                        line_memory.min(self.limits.max_record_bytes),
                        ErrorCode::ResourceMemory,
                        line_memory,
                        "inter-record FASTQ line",
                    )?;
                    if !trim_ascii(&line).is_empty() {
                        return Err(self.error(
                            ErrorCode::InputFastqStructure,
                            "expected a FASTQ '@' header at a record boundary",
                        ));
                    }
                }
            }
        }

        self.lines.begin_record()?;
        let (header_limit, header_limit_code) = self.header_line_limit()?;
        let header_line = self.lines.read_line(
            true,
            header_limit,
            header_limit_code,
            self.memory_cap,
            "FASTQ header",
        )?;
        let header = self.parse_header_line(&header_line, b'@', 0, header_line.capacity())?;
        drop(header_line);
        let mut sequence = Vec::new();
        let plus = loop {
            match self.lines.peek_byte()? {
                None => {
                    return Err(self.error(
                        ErrorCode::InputFastqStructure,
                        "FASTQ record ended before its '+' separator",
                    ));
                }
                Some(b'+') => {
                    let retained = checked_usize_sum(
                        header.identity.capacity(),
                        sequence.capacity(),
                        "FASTQ retained parser bytes",
                    )?;
                    let (separator_limit, separator_limit_code) = self.header_line_limit()?;
                    let memory_limit = self.line_memory_limit(retained)?;
                    let (separator_limit, separator_limit_code) = if memory_limit < separator_limit
                    {
                        (memory_limit, ErrorCode::ResourceMemory)
                    } else {
                        (separator_limit, separator_limit_code)
                    };
                    break self.lines.read_line(
                        true,
                        separator_limit,
                        separator_limit_code,
                        memory_limit,
                        "FASTQ '+' line",
                    )?;
                }
                Some(_) => {
                    let retained = checked_usize_sum(
                        header.identity.capacity(),
                        sequence.capacity(),
                        "FASTQ retained parser bytes",
                    )?;
                    let line_memory = self.line_memory_limit(retained)?;
                    let line = self.lines.read_line(
                        true,
                        line_memory,
                        ErrorCode::ResourceMemory,
                        line_memory,
                        "FASTQ sequence line",
                    )?;
                    let symbols = trim_ascii(&line);
                    if symbols.is_empty() {
                        return Err(self.error(
                            ErrorCode::InputFastqStructure,
                            "FASTQ record contains an empty sequence line",
                        ));
                    }
                    self.extend_sequence(
                        &mut sequence,
                        symbols,
                        header.identity.capacity(),
                        line.capacity(),
                    )?;
                }
            }
        };
        if sequence.is_empty() {
            return Err(self.error(
                ErrorCode::InputFastqStructure,
                "FASTQ record has an empty sequence",
            ));
        }
        self.enforce_live_memory(
            header.identity.capacity(),
            sequence.capacity(),
            0,
            plus.capacity(),
        )?;

        let plus_role = if plus.len() == 1 {
            None
        } else {
            let retained = checked_usize_sum(
                header.identity.capacity(),
                sequence.capacity(),
                "FASTQ retained parser bytes",
            )?;
            let retained = checked_usize_sum(retained, plus.capacity(), "FASTQ '+' parser bytes")?;
            let parsed = self.parse_header_payload(&plus[1..], retained)?;
            if parsed.identity != header.identity {
                return Err(self.error(
                    ErrorCode::InputFastqStructure,
                    "FASTQ '+' identifier does not match the '@' header",
                ));
            }
            parsed.role
        };
        drop(plus);
        if let (Some(header_role), Some(separator_role)) = (header.role, plus_role) {
            if header_role != separator_role {
                return Err(self.error(
                    ErrorCode::PairRole,
                    "FASTQ '+' mate role disagrees with the '@' header",
                ));
            }
        }
        let final_role = header.role.or(plus_role);

        let mut quality = Vec::new();
        while quality.len() < sequence.len() {
            let remaining = sequence.len() - quality.len();
            let remaining_u64 =
                u64::try_from(remaining).map_err(|_| overflow("FASTQ remaining quality length"))?;
            let retained = checked_usize_sum(
                header.identity.capacity(),
                sequence.capacity(),
                "FASTQ retained parser bytes",
            )?;
            let retained =
                checked_usize_sum(retained, quality.capacity(), "FASTQ quality parser bytes")?;
            let memory_line_limit = self.line_memory_limit(retained)?;
            let (quality_line_limit, quality_line_error) = if memory_line_limit < remaining_u64 {
                (memory_line_limit, ErrorCode::ResourceMemory)
            } else {
                (remaining_u64, ErrorCode::InputFastqStructure)
            };
            let line = match self.lines.peek_byte()? {
                None => {
                    return Err(self.error(
                        ErrorCode::InputFastqStructure,
                        "FASTQ record ended before quality data was complete",
                    ));
                }
                Some(_) => self.lines.read_line(
                    true,
                    quality_line_limit,
                    quality_line_error,
                    memory_line_limit,
                    "FASTQ quality line",
                )?,
            };
            if line.is_empty() {
                return Err(self.error(
                    ErrorCode::InputFastqStructure,
                    "FASTQ record contains an empty quality line",
                ));
            }
            for &value in &line {
                if !(33..=126).contains(&value) {
                    return Err(self.error(
                        ErrorCode::InputQuality,
                        format!("FASTQ quality byte {value} is outside Phred+33"),
                    ));
                }
            }
            let required_quality = quality
                .len()
                .checked_add(line.len())
                .ok_or_else(|| overflow("FASTQ quality length"))?;
            let quality_storage = if required_quality > quality.capacity() {
                quality
                    .capacity()
                    .checked_add(required_quality)
                    .ok_or_else(|| overflow("FASTQ quality reallocation peak"))?
            } else {
                quality.capacity()
            };
            self.enforce_live_memory(
                header.identity.capacity(),
                sequence.capacity(),
                quality_storage,
                line.capacity(),
            )?;
            quality.try_reserve_exact(line.len()).map_err(|_| {
                self.error(
                    ErrorCode::ResourceMemory,
                    "cannot reserve FASTQ quality memory",
                )
            })?;
            self.enforce_live_memory(
                header.identity.capacity(),
                sequence.capacity(),
                quality.capacity(),
                line.capacity(),
            )?;
            quality.extend_from_slice(&line);
        }
        if quality.len() != sequence.len() {
            return Err(self.error(
                ErrorCode::InputFastqStructure,
                "FASTQ sequence and quality lengths differ",
            ));
        }
        self.lines.end_record();
        let combined = ParsedHeader {
            identity: header.identity,
            role: final_role,
        };
        self.complete_record(combined, sequence, Some(quality))
            .map(Some)
    }

    fn complete_record(
        &mut self,
        header: ParsedHeader,
        sequence: Vec<u8>,
        quality: Option<Vec<u8>>,
    ) -> Result<ParsedRecord> {
        // Only paired streams have a mate role to infer. A role-free
        // single-end header is ordinary single-end input, not an inferred
        // mate-role observation.
        let inferred_role = self.expected_role != MateRole::S && header.role.is_none();
        if inferred_role {
            self.inferred_roles = self
                .inferred_roles
                .checked_add(1)
                .ok_or_else(|| overflow("inferred mate-role count"))?;
        }
        self.records = self
            .records
            .checked_add(1)
            .ok_or_else(|| overflow("source record count"))?;
        self.next_record_ordinal = self
            .next_record_ordinal
            .checked_add(1)
            .ok_or_else(|| overflow("source record ordinal"))?;
        self.bases = self
            .bases
            .checked_add(u64::try_from(sequence.len()).map_err(|_| overflow("source base count"))?)
            .ok_or_else(|| overflow("source base count"))?;
        Ok(ParsedRecord {
            normalized_identity_digest: normalized_identity_digest(&header.identity),
            normalized_identity: header.identity,
            sequence,
            quality,
            inferred_role,
        })
    }

    fn parse_header_line(
        &self,
        line: &[u8],
        marker: u8,
        other_live_bytes: usize,
        line_capacity: usize,
    ) -> Result<ParsedHeader> {
        if line.first() != Some(&marker) {
            return Err(self.error(ErrorCode::InputHeader, "record header marker is invalid"));
        }
        let payload_len = u64::try_from(line.len().saturating_sub(1))
            .map_err(|_| overflow("header payload length"))?;
        if payload_len > self.limits.max_header_bytes {
            return Err(self.error(
                ErrorCode::InputHeader,
                format!(
                    "header payload exceeds {} bytes",
                    self.limits.max_header_bytes
                ),
            ));
        }
        let live_with_line = checked_usize_sum(
            other_live_bytes,
            line_capacity,
            "header line live-memory accounting",
        )?;
        self.parse_header_payload(&line[1..], live_with_line)
    }

    fn parse_header_payload(
        &self,
        payload: &[u8],
        other_live_bytes: usize,
    ) -> Result<ParsedHeader> {
        let mut tokens = payload
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|token| !token.is_empty());
        let first = tokens
            .next()
            .ok_or_else(|| self.error(ErrorCode::InputHeader, "header has no identifier token"))?;
        let second = tokens.next();
        let (identity, slash_role) = terminal_slash_role(first);
        if identity.is_empty() {
            return Err(self.error(
                ErrorCode::InputHeader,
                "normalized header identity is empty",
            ));
        }
        let casava_role = second.and_then(casava_role);
        if let (Some(left), Some(right)) = (slash_role, casava_role) {
            if left != right {
                return Err(self.error(ErrorCode::PairRole, "slash and CASAVA mate roles disagree"));
            }
        }
        let role = slash_role.or(casava_role);
        if let (Some(stated), Some(expected)) = (role, paired_expected(self.expected_role)) {
            if stated != expected {
                return Err(self.error(
                    ErrorCode::PairRole,
                    format!(
                        "stated mate role {} disagrees with supplied {} stream",
                        stated.as_str(),
                        expected.as_str()
                    ),
                ));
            }
        }
        self.enforce_live_memory(other_live_bytes, 0, 0, identity.len())?;
        let mut owned_identity = Vec::new();
        owned_identity
            .try_reserve_exact(identity.len())
            .map_err(|_| self.error(ErrorCode::ResourceMemory, "cannot reserve header identity"))?;
        self.enforce_live_memory(other_live_bytes, 0, 0, owned_identity.capacity())?;
        owned_identity.extend_from_slice(identity);
        Ok(ParsedHeader {
            identity: owned_identity,
            role,
        })
    }

    fn extend_sequence(
        &self,
        sequence: &mut Vec<u8>,
        symbols: &[u8],
        identity_capacity: usize,
        line_capacity: usize,
    ) -> Result<()> {
        let next = sequence
            .len()
            .checked_add(symbols.len())
            .ok_or_else(|| overflow("read sequence length"))?;
        if u64::try_from(next).map_err(|_| overflow("read sequence length"))?
            > self.limits.max_read_bases
        {
            return Err(self.error(
                match self.format {
                    FastxFormat::Fasta => ErrorCode::InputFastaStructure,
                    FastxFormat::Fastq => ErrorCode::InputFastqStructure,
                },
                format!(
                    "sequence exceeds {} normalized bases",
                    self.limits.max_read_bases
                ),
            ));
        }
        let sequence_storage = if next > sequence.capacity() {
            sequence
                .capacity()
                .checked_add(next)
                .ok_or_else(|| overflow("sequence reallocation peak"))?
        } else {
            sequence.capacity()
        };
        self.enforce_live_memory(identity_capacity, sequence_storage, 0, line_capacity)?;
        for (position, &base) in symbols.iter().enumerate() {
            let upper = base.to_ascii_uppercase();
            if !is_iupac(upper) {
                return Err(self.error(
                    ErrorCode::InputNucleotide,
                    format!("unsupported nucleotide byte {base} at line offset {position}"),
                ));
            }
        }
        sequence.try_reserve_exact(symbols.len()).map_err(|_| {
            self.error(
                ErrorCode::ResourceMemory,
                "cannot reserve read sequence memory",
            )
        })?;
        self.enforce_live_memory(identity_capacity, sequence.capacity(), 0, line_capacity)?;
        sequence.extend(symbols.iter().map(u8::to_ascii_uppercase));
        Ok(())
    }

    fn enforce_live_memory(
        &self,
        identity: usize,
        sequence: usize,
        quality: usize,
        additional: usize,
    ) -> Result<()> {
        let total = identity
            .checked_add(sequence)
            .and_then(|value| value.checked_add(quality))
            .and_then(|value| value.checked_add(additional))
            .ok_or_else(|| overflow("parser live-memory accounting"))?;
        if u64::try_from(total).map_err(|_| overflow("parser live-memory accounting"))?
            > self.memory_cap
        {
            return Err(self.error(
                ErrorCode::ResourceMemory,
                format!(
                    "record exceeds parser memory allowance of {} bytes",
                    self.memory_cap
                ),
            ));
        }
        Ok(())
    }

    fn line_memory_limit(&self, retained_bytes: usize) -> Result<u64> {
        let retained = u64::try_from(retained_bytes)
            .map_err(|_| overflow("retained parser memory does not fit u64"))?;
        let available = self.memory_cap.checked_sub(retained).ok_or_else(|| {
            self.error(
                ErrorCode::ResourceMemory,
                format!(
                    "record exceeds parser memory allowance of {} bytes",
                    self.memory_cap
                ),
            )
        })?;
        Ok(available)
    }

    fn header_line_limit(&self) -> Result<(u64, ErrorCode)> {
        let configured = add_one(self.limits.max_header_bytes, "header line limit")?;
        Ok(if self.memory_cap < configured {
            (self.memory_cap, ErrorCode::ResourceMemory)
        } else {
            (configured, ErrorCode::InputHeader)
        })
    }

    fn error(&self, code: ErrorCode, context: impl std::fmt::Display) -> VeritasmError {
        VeritasmError::new(
            code,
            format!(
                "{} record {}: {context}",
                self.label,
                self.next_record_ordinal.saturating_add(1)
            ),
        )
    }
}

fn paired_expected(role: MateRole) -> Option<MateRole> {
    match role {
        MateRole::R1 | MateRole::R2 => Some(role),
        MateRole::S => None,
    }
}

fn terminal_slash_role(token: &[u8]) -> (&[u8], Option<MateRole>) {
    if let Some(identity) = token.strip_suffix(b"/1") {
        (identity, Some(MateRole::R1))
    } else if let Some(identity) = token.strip_suffix(b"/2") {
        (identity, Some(MateRole::R2))
    } else {
        (token, None)
    }
}

fn casava_role(token: &[u8]) -> Option<MateRole> {
    if token.starts_with(b"1:") {
        Some(MateRole::R1)
    } else if token.starts_with(b"2:") {
        Some(MateRole::R2)
    } else {
        None
    }
}

fn normalized_identity_digest(identity: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(NORMALIZED_ID_DOMAIN);
    hasher.update((identity.len() as u64).to_le_bytes());
    hasher.update(identity);
    hasher.finalize().into()
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |position| position + 1);
    &bytes[start..end]
}

fn is_iupac(base: u8) -> bool {
    matches!(
        base,
        b'A' | b'C'
            | b'G'
            | b'T'
            | b'R'
            | b'Y'
            | b'S'
            | b'W'
            | b'K'
            | b'M'
            | b'B'
            | b'D'
            | b'H'
            | b'V'
            | b'N'
    )
}

fn add_one(value: u64, context: &'static str) -> Result<u64> {
    value.checked_add(1).ok_or_else(|| overflow(context))
}

fn checked_usize_sum(left: usize, right: usize, context: &'static str) -> Result<usize> {
    left.checked_add(right).ok_or_else(|| overflow(context))
}

struct BoundedLines<R: Read> {
    reader: BufReader<R>,
    max_record_bytes: u64,
    current_record_bytes: Option<u64>,
    label: String,
}

impl<R: Read> BoundedLines<R> {
    fn new(reader: R, max_record_bytes: u64, label: String) -> Self {
        Self {
            reader: BufReader::with_capacity(READER_BUFFER_BYTES, reader),
            max_record_bytes,
            current_record_bytes: None,
            label,
        }
    }

    fn peek_byte(&mut self) -> Result<Option<u8>> {
        let bytes = self.reader.fill_buf().map_err(|error| {
            VeritasmError::new(
                ErrorCode::InputRead,
                format!("{}: cannot read decoded source: {error}", self.label),
            )
        })?;
        Ok(bytes.first().copied())
    }

    fn begin_record(&mut self) -> Result<()> {
        if self.current_record_bytes.is_some() {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                format!("{}: nested FASTX record accounting", self.label),
            ));
        }
        self.current_record_bytes = Some(0);
        Ok(())
    }

    fn end_record(&mut self) {
        self.current_record_bytes = None;
    }

    fn read_line(
        &mut self,
        in_record: bool,
        max_content_bytes: u64,
        cap_error: ErrorCode,
        max_allocation_bytes: u64,
        kind: &'static str,
    ) -> Result<Vec<u8>> {
        if in_record != self.current_record_bytes.is_some() {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                format!(
                    "{}: invalid record-span state while reading {kind}",
                    self.label
                ),
            ));
        }
        let mut result = Vec::new();
        let mut saw_newline = false;
        let mut pending_carriage_return = false;
        loop {
            let available = self.reader.fill_buf().map_err(|error| {
                VeritasmError::new(
                    ErrorCode::InputRead,
                    format!("{}: cannot read {kind}: {error}", self.label),
                )
            })?;
            if available.is_empty() {
                if pending_carriage_return {
                    let projected = u64::try_from(result.len())
                        .map_err(|_| overflow("line content length"))?
                        .checked_add(1)
                        .ok_or_else(|| overflow("line content length"))?;
                    if projected > max_content_bytes {
                        return Err(VeritasmError::new(
                            cap_error,
                            format!(
                                "{}: {kind} exceeds {max_content_bytes} content bytes",
                                self.label
                            ),
                        ));
                    }
                    reserve_line_capacity(&mut result, 1, max_allocation_bytes, kind, &self.label)?;
                    result.push(b'\r');
                }
                break;
            }
            let count = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |position| position + 1);
            let count_u64 = u64::try_from(count).map_err(|_| overflow("physical line chunk"))?;
            if let Some(current) = self.current_record_bytes {
                let next = current
                    .checked_add(count_u64)
                    .ok_or_else(|| overflow("decoded record span"))?;
                if next > self.max_record_bytes {
                    let code = if kind.contains("FASTA") {
                        ErrorCode::InputFastaStructure
                    } else {
                        ErrorCode::InputFastqStructure
                    };
                    return Err(VeritasmError::new(
                        code,
                        format!(
                            "{}: decoded record span exceeds {} bytes",
                            self.label, self.max_record_bytes
                        ),
                    ));
                }
                self.current_record_bytes = Some(next);
            }

            let mut projected =
                u64::try_from(result.len()).map_err(|_| overflow("line content length"))?;
            let mut projected_pending = pending_carriage_return;
            for &byte in &available[..count] {
                if byte == b'\n' {
                    projected_pending = false;
                    break;
                }
                if projected_pending {
                    projected = projected
                        .checked_add(1)
                        .ok_or_else(|| overflow("line content length"))?;
                    projected_pending = false;
                }
                if byte == b'\r' {
                    projected_pending = true;
                } else {
                    projected = projected
                        .checked_add(1)
                        .ok_or_else(|| overflow("line content length"))?;
                }
                if projected > max_content_bytes {
                    return Err(VeritasmError::new(
                        cap_error,
                        format!(
                            "{}: {kind} exceeds {max_content_bytes} content bytes",
                            self.label
                        ),
                    ));
                }
            }
            let projected = usize::try_from(projected)
                .map_err(|_| overflow("line allocation does not fit usize"))?;
            let additional = projected
                .checked_sub(result.len())
                .ok_or_else(|| overflow("line allocation projection"))?;
            reserve_line_capacity(
                &mut result,
                additional,
                max_allocation_bytes,
                kind,
                &self.label,
            )?;

            for &byte in &available[..count] {
                if byte == b'\n' {
                    pending_carriage_return = false;
                    saw_newline = true;
                    break;
                }
                if pending_carriage_return {
                    result.push(b'\r');
                    pending_carriage_return = false;
                }
                if byte == b'\r' {
                    pending_carriage_return = true;
                } else {
                    result.push(byte);
                }
            }
            debug_assert_eq!(pending_carriage_return, projected_pending);
            self.reader.consume(count);
            if saw_newline {
                break;
            }
        }
        Ok(result)
    }
}

fn reserve_line_capacity(
    result: &mut Vec<u8>,
    additional: usize,
    max_allocation_bytes: u64,
    kind: &'static str,
    label: &str,
) -> Result<()> {
    let required = result
        .len()
        .checked_add(additional)
        .ok_or_else(|| overflow("line allocation projection"))?;
    if required <= result.capacity() {
        return Ok(());
    }
    let transient_peak = result
        .capacity()
        .checked_add(required)
        .ok_or_else(|| overflow("line reallocation peak"))?;
    if u64::try_from(transient_peak).map_err(|_| overflow("line reallocation peak"))?
        > max_allocation_bytes
    {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{}: {kind} reallocation exceeds {max_allocation_bytes} bytes",
                label
            ),
        ));
    }
    let old_capacity = result.capacity();
    result.try_reserve_exact(additional).map_err(|_| {
        VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!("{}: cannot reserve memory for {kind}", label),
        )
    })?;
    let retained_peak = old_capacity
        .checked_add(result.capacity())
        .ok_or_else(|| overflow("line retained reallocation peak"))?;
    if u64::try_from(retained_peak).map_err(|_| overflow("line retained reallocation peak"))?
        > max_allocation_bytes
    {
        return Err(VeritasmError::new(
            ErrorCode::ResourceMemory,
            format!(
                "{}: {kind} allocator capacity exceeds {max_allocation_bytes} bytes",
                label
            ),
        ));
    }
    Ok(())
}

impl BoundedLines<DecodedReader> {
    fn authenticate_eof(&mut self) -> Result<()> {
        let buffered = self.reader.fill_buf().map_err(|error| {
            VeritasmError::new(
                ErrorCode::InputRead,
                format!(
                    "{}: cannot authenticate decoded source EOF: {error}",
                    self.label
                ),
            )
        })?;
        if !buffered.is_empty() {
            return Err(VeritasmError::new(
                ErrorCode::InternalInvariant,
                format!(
                    "{}: attempted decoded-source authentication before EOF",
                    self.label
                ),
            ));
        }
        self.reader.get_mut().finish_authentication()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{prepare_source, SourceSpec};
    use std::fs::{self, OpenOptions};
    use std::io::{Seek, SeekFrom, Write};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn reader_for(bytes: &[u8], role: MateRole) -> (tempfile::TempDir, PreparedSource, Limits) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("input.fastx");
        fs::write(&path, bytes).unwrap();
        let spec = SourceSpec::new(0, role, path).unwrap();
        let limits = Limits::default();
        let source = prepare_source(spec, &limits, directory.path(), 0, &mut 0).unwrap();
        (directory, source, limits)
    }

    fn overwrite_decoded(source: &PreparedSource, replacement: &[u8]) {
        assert_eq!(replacement.len() as u64, source.decoded_bytes);
        #[cfg(unix)]
        fs::set_permissions(
            source.decoded_path_for_test(),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let mut file = OpenOptions::new()
            .write(true)
            .open(source.decoded_path_for_test())
            .unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(replacement).unwrap();
        file.flush().unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn rejects_same_length_decoded_mutation_before_parser_open() {
        let original = b"@read\nACGT\n+\nIIII\n";
        let replacement = b"@read\nTGCA\n+\nIIII\n";
        let (_directory, source, limits) = reader_for(original, MateRole::S);
        overwrite_decoded(&source, replacement);

        let error = match FastxReader::open(&source, &limits, 64 << 20) {
            Err(error) => error,
            Ok(mut reader) => reader.next_record().unwrap_err(),
        };
        assert_eq!(error.code(), ErrorCode::InputRead);
    }

    #[test]
    fn final_record_is_withheld_after_mutation_before_traversal() {
        let original = b"@read\nACGT\n+\nIIII\n";
        let replacement = b"@read\nTGCA\n+\nIIII\n";
        let (_directory, source, limits) = reader_for(original, MateRole::S);
        let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
        assert!(!reader.is_authenticated());
        overwrite_decoded(&source, replacement);

        let error = reader.next_record().unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputRead);
        assert!(!reader.is_authenticated());
    }

    #[test]
    fn final_record_is_withheld_after_mutation_between_records() {
        let original = b"@one\nAAAA\n+\nIIII\n@two\nCCCC\n+\nIIII\n";
        let replacement = b"@one\nAAAA\n+\nIIII\n@two\nGGGG\n+\nIIII\n";
        let (_directory, source, limits) = reader_for(original, MateRole::S);
        let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
        assert_eq!(reader.next_record().unwrap().unwrap().sequence, b"AAAA");
        assert!(!reader.is_authenticated());
        overwrite_decoded(&source, replacement);

        let error = reader.next_record().unwrap_err();
        assert_eq!(error.code(), ErrorCode::InputRead);
        assert!(!reader.is_authenticated());
    }

    #[test]
    fn parses_wrapped_fasta_and_arbitrary_header_bytes() {
        let (_directory, source, limits) =
            reader_for(b"\n>id\xff note\n acg \n\tNN\t\n", MateRole::S);
        let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
        let record = reader.next_record().unwrap().unwrap();
        assert_eq!(record.normalized_identity, b"id\xff");
        assert_eq!(record.sequence, b"ACGNN");
        assert!(reader.next_record().unwrap().is_none());
    }

    #[test]
    fn parses_wrapped_fastq_and_checks_plus_role() {
        let (_directory, source, limits) = reader_for(
            b"@x/1 1:N:0:1\nAC\nGT\n+x/1 1:N:0:1\nII\nII\n",
            MateRole::R1,
        );
        let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
        let record = reader.next_record().unwrap().unwrap();
        assert_eq!(record.normalized_identity, b"x");
        assert_eq!(record.sequence, b"ACGT");
        assert_eq!(record.quality.as_deref(), Some(b"IIII".as_slice()));
        assert!(!record.inferred_role);
    }

    #[test]
    fn role_free_headers_count_inference_only_for_paired_streams() {
        let (_single_directory, single_source, single_limits) =
            reader_for(b"@x\nACGT\n+\nIIII\n", MateRole::S);
        let mut single = FastxReader::open(&single_source, &single_limits, 64 << 20).unwrap();
        assert!(!single.next_record().unwrap().unwrap().inferred_role);
        assert_eq!(single.inferred_roles(), 0);

        let (_paired_directory, paired_source, paired_limits) =
            reader_for(b"@x\nACGT\n+\nIIII\n", MateRole::R1);
        let mut paired = FastxReader::open(&paired_source, &paired_limits, 64 << 20).unwrap();
        assert!(paired.next_record().unwrap().unwrap().inferred_role);
        assert_eq!(paired.inferred_roles(), 1);
    }

    #[test]
    fn rejects_blank_fastq_sequence_line() {
        let (_directory, source, limits) = reader_for(b"@x\n\n+\n", MateRole::S);
        let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
        assert_eq!(
            reader.next_record().unwrap_err().code(),
            ErrorCode::InputFastqStructure
        );
    }

    #[test]
    fn record_limit_counts_markers_and_line_endings() {
        let (_directory, source, mut limits) = reader_for(b">x\r\nAC\r\n", MateRole::S);
        limits.max_record_bytes = 7;
        let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
        assert_eq!(
            reader.next_record().unwrap_err().code(),
            ErrorCode::InputFastaStructure
        );
    }

    #[test]
    fn header_payload_limit_accepts_limit_and_rejects_limit_plus_one() {
        const LIMIT: usize = 8;
        for payload_length in [LIMIT - 1, LIMIT, LIMIT + 1] {
            let mut bytes = vec![b'>'];
            bytes.extend(std::iter::repeat_n(b'x', payload_length));
            bytes.extend_from_slice(b"\r\nACG\n");
            let (_directory, source, mut limits) = reader_for(&bytes, MateRole::S);
            limits.max_header_bytes = LIMIT as u64;
            let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
            if payload_length <= LIMIT {
                let record = reader.next_record().unwrap().unwrap();
                assert_eq!(record.normalized_identity.len(), payload_length);
            } else {
                assert_eq!(
                    reader.next_record().unwrap_err().code(),
                    ErrorCode::InputHeader
                );
            }
        }
    }

    #[test]
    fn normalized_read_limit_accepts_limit_and_rejects_limit_plus_one() {
        const LIMIT: usize = 8;
        for read_length in [LIMIT - 1, LIMIT, LIMIT + 1] {
            let mut bytes = b">x\n".to_vec();
            bytes.extend(std::iter::repeat_n(b'A', read_length));
            bytes.push(b'\n');
            let (_directory, source, mut limits) = reader_for(&bytes, MateRole::S);
            limits.max_read_bases = LIMIT as u64;
            let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
            if read_length <= LIMIT {
                assert_eq!(
                    reader.next_record().unwrap().unwrap().sequence.len(),
                    read_length
                );
            } else {
                assert_eq!(
                    reader.next_record().unwrap_err().code(),
                    ErrorCode::InputFastaStructure
                );
            }
        }
    }

    #[test]
    fn decoded_record_span_accepts_limit_and_rejects_limit_plus_one() {
        const LIMIT: usize = 12;
        for record_span in [LIMIT - 1, LIMIT, LIMIT + 1] {
            let sequence_length = record_span - 4;
            let mut bytes = b">x\n".to_vec();
            bytes.extend(std::iter::repeat_n(b'A', sequence_length));
            bytes.push(b'\n');
            assert_eq!(bytes.len(), record_span);
            let (_directory, source, mut limits) = reader_for(&bytes, MateRole::S);
            limits.max_record_bytes = LIMIT as u64;
            let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
            if record_span <= LIMIT {
                assert_eq!(
                    reader.next_record().unwrap().unwrap().sequence.len(),
                    sequence_length
                );
            } else {
                assert_eq!(
                    reader.next_record().unwrap_err().code(),
                    ErrorCode::InputFastaStructure
                );
            }
        }
    }

    #[test]
    fn quality_line_is_capped_before_append_and_must_exactly_match() {
        const LIMIT: usize = 8;
        for quality_length in [LIMIT - 1, LIMIT, LIMIT + 1] {
            let mut bytes = b"@x\nAAAAAAAA\n+\n".to_vec();
            bytes.extend(std::iter::repeat_n(b'I', quality_length));
            bytes.push(b'\n');
            let (_directory, source, mut limits) = reader_for(&bytes, MateRole::S);
            limits.max_read_bases = LIMIT as u64;
            let mut reader = FastxReader::open(&source, &limits, 64 << 20).unwrap();
            if quality_length == LIMIT {
                assert_eq!(
                    reader
                        .next_record()
                        .unwrap()
                        .unwrap()
                        .quality
                        .unwrap()
                        .len(),
                    quality_length
                );
            } else {
                assert_eq!(
                    reader.next_record().unwrap_err().code(),
                    ErrorCode::InputFastqStructure
                );
            }
        }
    }

    #[test]
    fn adversarial_physical_line_stops_at_small_content_cap() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("long-line");
        let mut bytes = vec![b'A'; READER_BUFFER_BYTES * 4];
        bytes.push(b'\n');
        fs::write(&path, bytes).unwrap();
        let file = File::open(path).unwrap();
        let mut lines = BoundedLines::new(file, u64::MAX, "adversarial".to_owned());
        let error = lines
            .read_line(false, 31, ErrorCode::ResourceMemory, 31, "adversarial line")
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ResourceMemory);
    }

    #[test]
    fn crlf_terminator_does_not_consume_content_allowance() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("crlf");
        fs::write(&path, b"AAAA\r\nBBBBB\nCCCC\r").unwrap();
        let file = File::open(path).unwrap();
        let mut lines = BoundedLines::new(file, u64::MAX, "crlf".to_owned());
        assert_eq!(
            lines
                .read_line(false, 4, ErrorCode::ResourceMemory, 4, "line")
                .unwrap(),
            b"AAAA"
        );
        assert_eq!(
            lines
                .read_line(false, 4, ErrorCode::ResourceMemory, 4, "line")
                .unwrap_err()
                .code(),
            ErrorCode::ResourceMemory
        );
    }

    #[test]
    fn parser_memory_cap_accounts_for_line_and_destination_overlap() {
        let (_directory, source, limits) = reader_for(b"@x\nAAAA\n+\nIIII\n", MateRole::S);
        let mut exact = FastxReader::open(&source, &limits, 13).unwrap();
        assert!(exact.next_record().unwrap().is_some());

        let mut too_small = FastxReader::open(&source, &limits, 12).unwrap();
        assert_eq!(
            too_small.next_record().unwrap_err().code(),
            ErrorCode::ResourceMemory
        );
    }
}

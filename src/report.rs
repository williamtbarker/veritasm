//! Deterministic, self-contained HTML rendering from bounded typed summaries.

use crate::model::{AvailabilityU64, PairLaneStateCount, Unitig, WindowStats};
use std::io::{self, Write};

/// Values already validated at the bundle boundary. The report owns no copy of
/// the assembly or `run.json`; it streams at most `maximum_rows` unitig rows.
pub(crate) struct ReportData<'a> {
    pub status_code: &'a str,
    pub status_message: &'a str,
    pub input_mode: &'a str,
    pub fragments: u64,
    pub reads: u64,
    pub bases: u64,
    pub raw_transport_bytes: u64,
    pub max_raw_transport_bytes: u64,
    pub decoded_input_bytes: u64,
    pub inferred_mate_roles: u64,
    pub gzip_sources: u64,
    pub gzip_members: u64,
    pub max_gzip_members: u64,
    pub windows: &'a WindowStats,
    pub k: u8,
    pub profile: &'a str,
    pub support_unit: &'a str,
    pub retention_min_support: u64,
    pub min_base_quality: u8,
    pub remap: bool,
    pub retained_canonical_keys: u64,
    pub linear_unitigs: u64,
    pub closed_graph_walks: u64,
    pub enumeration_status: &'a str,
    pub pair_mode: &'a str,
    pub pair_lane_states: &'a [PairLaneStateCount],
    pub mapper_algorithm_id: &'a str,
    pub mapper_algorithm_version: &'a str,
    pub mapper_execution_status: &'a str,
    pub mapper_seed_length: u8,
    pub mapper_linear_targets: u64,
    pub mapper_index_postings: u64,
    pub mapper_accounted_index_bytes: u64,
    pub mapper_target_universe: &'a str,
    pub scientific_scope: &'a str,
    pub evidence_source: &'a str,
    pub evidence_interpretation: &'a str,
    pub biological_call: &'a str,
    pub taxonomy: &'a str,
    pub control_context: &'a str,
    pub unitigs: &'a [Unitig],
    pub maximum_rows: u64,
    pub limitations: &'a [&'a str],
    pub disclaimer: &'a str,
}

/// Stream a self-contained report without materializing HTML or reparsing the
/// potentially large support histogram in `run.json`.
pub(crate) fn write_html(writer: &mut dyn Write, data: &ReportData<'_>) -> io::Result<()> {
    let row_limit = usize::try_from(data.maximum_rows).unwrap_or(usize::MAX);
    let shown = data.unitigs.len().min(row_limit);
    let omitted = data.unitigs.len().saturating_sub(shown);

    writer.write_all(b"<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n")?;
    writer
        .write_all(b"<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n")?;
    writer.write_all(b"<title>VeritAsm evidence report</title>\n<style>body{font-family:system-ui,sans-serif;max-width:76rem;margin:2rem auto;padding:0 1rem;color:#18202a}h1,h2{line-height:1.2}table{border-collapse:collapse;width:100%;margin:1rem 0}th,td{border:1px solid #bbc3cc;padding:.45rem;text-align:left;vertical-align:top}th{background:#eef2f5}.mono{font-family:ui-monospace,monospace;overflow-wrap:anywhere}.notice{border-left:.35rem solid #596b7c;padding:.8rem 1rem;background:#f4f6f8}.na{font-style:italic}</style>\n</head><body>\n")?;
    writer.write_all(b"<h1>VeritAsm evidence report</h1>\n<div class=\"notice\"><strong>Technical status:</strong> ")?;
    write_escaped(writer, data.status_code)?;
    writer.write_all(b"<br>")?;
    write_escaped(writer, data.status_message)?;
    writer.write_all(b"</div>\n<h2>Run summary</h2>\n<table><tbody>")?;
    row_text(writer, "Input mode", data.input_mode)?;
    row_u64(writer, "Supplied fragments", data.fragments)?;
    row_u64(writer, "Read instances", data.reads)?;
    row_u64(writer, "Input bases", data.bases)?;
    row_u64(writer, "Raw transport bytes", data.raw_transport_bytes)?;
    row_u64(
        writer,
        "Maximum raw transport bytes",
        data.max_raw_transport_bytes,
    )?;
    row_u64(writer, "Decoded input bytes", data.decoded_input_bytes)?;
    row_u64(writer, "Inferred mate roles", data.inferred_mate_roles)?;
    row_u64(writer, "Gzip sources", data.gzip_sources)?;
    row_u64(writer, "Gzip members", data.gzip_members)?;
    row_u64(writer, "Maximum gzip members", data.max_gzip_members)?;
    row_u64(writer, "Possible k-mer windows", data.windows.possible)?;
    row_u64(writer, "Accepted windows", data.windows.accepted)?;
    row_u64(
        writer,
        "Ambiguity-only rejections",
        data.windows.ambiguity_only,
    )?;
    row_u64(writer, "Quality-only rejections", data.windows.quality_only)?;
    row_u64(
        writer,
        "Ambiguity-and-quality rejections",
        data.windows.ambiguity_and_quality,
    )?;
    row_u64(
        writer,
        "Retained canonical k-mers",
        data.retained_canonical_keys,
    )?;
    row_u64(writer, "Linear unitigs", data.linear_unitigs)?;
    row_u64(writer, "Closed graph walks", data.closed_graph_walks)?;
    row_text(writer, "Placement enumeration", data.enumeration_status)?;
    row_text(writer, "Pair audit mode", data.pair_mode)?;
    writer.write_all(b"</tbody></table>\n<h2>Pair audit by lane</h2>\n<table><thead><tr><th>Lane ordinal</th><th>State</th><th>Supplied fragment instances</th></tr></thead><tbody>\n")?;
    for state in data.pair_lane_states {
        writer.write_all(b"<tr><td>")?;
        write!(writer, "{}", state.lane_ordinal)?;
        writer.write_all(b"</td><td>")?;
        write_escaped(writer, state.state)?;
        writeln!(writer, "</td><td>{}</td></tr>", state.count)?;
    }
    writer.write_all(b"</tbody></table>\n<h2>Scientific parameters</h2>\n<table><tbody>")?;
    row_u64(writer, "k", u64::from(data.k))?;
    row_text(writer, "Profile", data.profile)?;
    row_text(writer, "Support unit", data.support_unit)?;
    row_u64(
        writer,
        "Retention minimum support",
        data.retention_min_support,
    )?;
    row_u64(
        writer,
        "Minimum base quality",
        u64::from(data.min_base_quality),
    )?;
    row_text(
        writer,
        "Construction-read remapping requested",
        bool_text(data.remap),
    )?;
    row_text(writer, "Mapper algorithm", data.mapper_algorithm_id)?;
    row_text(
        writer,
        "Mapper algorithm version",
        data.mapper_algorithm_version,
    )?;
    row_text(
        writer,
        "Mapper execution status",
        data.mapper_execution_status,
    )?;
    row_u64(
        writer,
        "Mapper literal seed length",
        u64::from(data.mapper_seed_length),
    )?;
    row_u64(writer, "Mapper linear targets", data.mapper_linear_targets)?;
    row_u64(writer, "Mapper index postings", data.mapper_index_postings)?;
    row_u64(
        writer,
        "Mapper accounted index bytes",
        data.mapper_accounted_index_bytes,
    )?;
    row_text(
        writer,
        "Mapping target universe",
        data.mapper_target_universe,
    )?;
    writer.write_all(b"</tbody></table>\n<p class=\"notice\">Construction-read remapping searches emitted linear unitigs only; closed graph walks are not mapping targets.</p>\n<h2>Interpretation</h2>\n<table><tbody>")?;
    row_text(writer, "scientific_scope", data.scientific_scope)?;
    row_text(writer, "evidence_source", data.evidence_source)?;
    row_text(
        writer,
        "evidence_interpretation",
        data.evidence_interpretation,
    )?;
    row_text(writer, "support_unit", data.support_unit)?;
    row_text(writer, "biological_call", data.biological_call)?;
    row_text(writer, "taxonomy", data.taxonomy)?;
    row_text(writer, "control_context", data.control_context)?;
    writer.write_all(b"</tbody></table>\n<h2>Unitig evidence</h2>\n")?;
    writeln!(
        writer,
        "<p>Showing {} of {} deterministic rows; {} omitted by the recorded presentation limit.</p>",
        shown,
        data.unitigs.len(),
        omitted
    )?;
    writer.write_all(b"<table><thead><tr><th>ID</th><th>Length</th><th>Topology</th><th>Canonical k-mers</th><th>Support min / median / max</th><th>Read placements</th><th>Placement status</th></tr></thead><tbody>\n")?;
    for unitig in data.unitigs.iter().take(shown) {
        writer.write_all(b"<tr><td class=\"mono\">")?;
        write_escaped(writer, &unitig.id)?;
        write!(
            writer,
            "</td><td>{}</td><td>{}</td><td>{}</td><td>{} / {} / {}</td><td",
            unitig.sequence.len(),
            unitig.topology.as_str(),
            unitig.canonical_kmers,
            unitig.minimum_support,
            unitig.lower_median_support,
            unitig.maximum_support
        )?;
        if matches!(
            unitig.enumeration_complete_read_placements,
            AvailabilityU64::NotAvailable(_)
        ) {
            writer.write_all(b" class=\"na\"")?;
        }
        writer.write_all(b">")?;
        write_availability(writer, &unitig.enumeration_complete_read_placements)?;
        writer.write_all(b"</td><td>")?;
        write_escaped(writer, unitig.placement_enumeration_status)?;
        writer.write_all(b"</td></tr>\n")?;
    }
    writer.write_all(b"</tbody></table>\n<h2>Limitations</h2><ol>\n")?;
    for limitation in data.limitations {
        writer.write_all(b"<li>")?;
        write_escaped(writer, limitation)?;
        writer.write_all(b"</li>\n")?;
    }
    writer.write_all(b"</ol>\n<h2>Scope disclaimer</h2><p class=\"notice\">")?;
    write_escaped(writer, data.disclaimer)?;
    writer.write_all(b"</p>\n</body></html>\n")
}

fn row_text(writer: &mut dyn Write, name: &str, value: &str) -> io::Result<()> {
    writer.write_all(b"<tr><th>")?;
    write_escaped(writer, name)?;
    writer.write_all(b"</th><td>")?;
    write_escaped(writer, value)?;
    writer.write_all(b"</td></tr>\n")
}

fn row_u64(writer: &mut dyn Write, name: &str, value: u64) -> io::Result<()> {
    writer.write_all(b"<tr><th>")?;
    write_escaped(writer, name)?;
    writeln!(writer, "</th><td>{value}</td></tr>")
}

const fn bool_text(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn write_availability(writer: &mut dyn Write, value: &AvailabilityU64) -> io::Result<()> {
    match value {
        AvailabilityU64::Value(value) => write!(writer, "{value}"),
        AvailabilityU64::NotAvailable(reason) => {
            writer.write_all(b"NA (")?;
            write_escaped(writer, reason)?;
            writer.write_all(b")")
        }
    }
}

fn write_escaped(writer: &mut dyn Write, value: &str) -> io::Result<()> {
    for character in value.chars() {
        match character {
            '&' => writer.write_all(b"&amp;")?,
            '<' => writer.write_all(b"&lt;")?,
            '>' => writer.write_all(b"&gt;")?,
            '"' => writer.write_all(b"&quot;")?,
            '\'' => writer.write_all(b"&#39;")?,
            _ => write!(writer, "{character}")?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Topology;

    const TEST_WINDOWS: WindowStats = WindowStats {
        possible: 1,
        accepted: 1,
        ambiguity_only: 0,
        quality_only: 0,
        ambiguity_and_quality: 0,
    };
    const TEST_PAIR_LANE_STATES: [PairLaneStateCount; 1] = [PairLaneStateCount {
        lane_ordinal: 0,
        state: "not_paired_input",
        count: 1,
    }];

    fn data<'a>(unitigs: &'a [Unitig], limitations: &'a [&'a str]) -> ReportData<'a> {
        ReportData {
            status_code: "software_run_complete",
            status_message: "<img src=x>",
            input_mode: "single_end",
            fragments: 1,
            reads: 1,
            bases: 3,
            raw_transport_bytes: 6,
            max_raw_transport_bytes: 1 << 20,
            decoded_input_bytes: 6,
            inferred_mate_roles: 0,
            gzip_sources: 0,
            gzip_members: 0,
            max_gzip_members: 8,
            windows: &TEST_WINDOWS,
            k: 3,
            profile: "retain_all",
            support_unit: "supplied_fragment_instance",
            retention_min_support: 1,
            min_base_quality: 20,
            remap: true,
            retained_canonical_keys: 1,
            linear_unitigs: 1,
            closed_graph_walks: 0,
            enumeration_status: "placement_enumeration_complete",
            pair_mode: "not_paired_input",
            pair_lane_states: &TEST_PAIR_LANE_STATES,
            mapper_algorithm_id: "literal_rarest_seed_zero_mismatch",
            mapper_algorithm_version: "1",
            mapper_execution_status: "executed",
            mapper_seed_length: 15,
            mapper_linear_targets: 1,
            mapper_index_postings: 0,
            mapper_accounted_index_bytes: 73_773,
            mapper_target_universe: "all_emitted_linear_unitigs_only",
            scientific_scope: "algorithmic_unitig_reconstruction",
            evidence_source: "construction_reads",
            evidence_interpretation: "internal_consistency_not_independent_validation",
            biological_call: "not_performed",
            taxonomy: "not_performed",
            control_context: "not_supplied",
            unitigs,
            maximum_rows: 1,
            limitations,
            disclaimer: "safe & scoped",
        }
    }

    #[test]
    fn escapes_every_html_metacharacter() {
        let mut output = Vec::new();
        write_escaped(&mut output, "<&>\"'").unwrap();
        assert_eq!(output, b"&lt;&amp;&gt;&quot;&#39;");
    }

    #[test]
    fn dynamic_values_cannot_inject_html() {
        let unitig = Unitig {
            id: "<svg onload=alert(1)>".to_owned(),
            sequence: b"AAC".to_vec(),
            topology: Topology::Linear,
            edge_steps: 1,
            canonical_kmers: 1,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::Value(1),
            single_group_read_instances: AvailabilityU64::Value(1),
            multi_group_read_instances_with_group: AvailabilityU64::Value(0),
            placement_enumeration_status: "placement_enumeration_complete",
            sequence_sha256: "0".repeat(64),
        };
        let mut html = Vec::new();
        write_html(&mut html, &data(&[unitig], &["<script>alert(1)</script>"])).unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(!html.contains("<img src=x>"));
        assert!(!html.contains("<script>alert"));
        assert!(!html.contains("<svg onload"));
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&lt;svg onload=alert(1)&gt;"));
    }

    #[test]
    fn presentation_limit_bounds_emitted_unitig_rows() {
        let make_unitig = |id: &str| Unitig {
            id: id.to_owned(),
            sequence: b"AAC".to_vec(),
            topology: Topology::Linear,
            edge_steps: 1,
            canonical_kmers: 1,
            minimum_support: 1,
            lower_median_support: 1,
            maximum_support: 1,
            enumeration_complete_read_placements: AvailabilityU64::Value(1),
            single_group_read_instances: AvailabilityU64::Value(1),
            multi_group_read_instances_with_group: AvailabilityU64::Value(0),
            placement_enumeration_status: "placement_enumeration_complete",
            sequence_sha256: "0".repeat(64),
        };
        let unitigs = [make_unitig("first-row"), make_unitig("omitted-row")];
        let mut html = Vec::new();
        write_html(&mut html, &data(&unitigs, &[])).unwrap();
        let html = String::from_utf8(html).unwrap();
        assert!(html.contains("first-row"));
        assert!(!html.contains("omitted-row"));
        assert!(html.contains("Showing 1 of 2 deterministic rows; 1 omitted"));
    }

    #[test]
    fn renders_scientific_parameters_interpretation_and_linear_target_scope() {
        let mut html = Vec::new();
        write_html(&mut html, &data(&[], &[])).unwrap();
        let html = String::from_utf8(html).unwrap();
        for expected in [
            "retain_all",
            "supplied_fragment_instance",
            "literal_rarest_seed_zero_mismatch",
            "all_emitted_linear_unitigs_only",
            "algorithmic_unitig_reconstruction",
            "construction_reads",
            "internal_consistency_not_independent_validation",
            "not_performed",
            "not_supplied",
            "closed graph walks are not mapping targets",
            "safe &amp; scoped",
        ] {
            assert!(html.contains(expected), "missing report value: {expected}");
        }
        assert!(html.contains("<th>k</th><td>3</td>"));
        assert!(html.contains("<th>Raw transport bytes</th><td>6</td>"));
        assert!(html.contains("<th>Maximum raw transport bytes</th><td>1048576</td>"));
        assert!(html.contains("<th>Decoded input bytes</th><td>6</td>"));
        assert!(html.contains("<th>Maximum gzip members</th><td>8</td>"));
        assert!(html.contains("<th>Minimum base quality</th><td>20</td>"));
        assert!(html.contains("<th>Retention minimum support</th><td>1</td>"));
        assert!(html.contains("<th>Mapper execution status</th><td>executed</td>"));
        assert!(html.contains("<th>Mapper literal seed length</th><td>15</td>"));
        assert!(html.contains("<th>Mapper linear targets</th><td>1</td>"));
        assert!(html.contains("<th>Mapper index postings</th><td>0</td>"));
        assert!(html.contains("<th>Mapper accounted index bytes</th><td>73773</td>"));
        assert!(html.contains("<h2>Pair audit by lane</h2>"));
        assert!(html.contains("<tr><td>0</td><td>not_paired_input</td><td>1</td></tr>"));
    }
}

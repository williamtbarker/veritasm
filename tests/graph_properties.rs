use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use veritasm::compact::compact_graph;
use veritasm::graph::ExactGraph;
use veritasm::model::{GraphLink, KmerCount, Topology};

const TEST_MEMORY_BUDGET: u64 = 64 << 20;

fn oriented(sequence: &[u8], orientation: char) -> Vec<u8> {
    if orientation == '+' {
        return sequence.to_vec();
    }
    sequence
        .iter()
        .rev()
        .map(|base| match base {
            b'A' => b'T',
            b'C' => b'G',
            b'G' => b'C',
            b'T' => b'A',
            _ => panic!("assembled sequences contain only ACGT"),
        })
        .collect()
}

// Deliberately independent of the production DNA module.
fn reverse_complement_code(mut code: u128, length: u8) -> u128 {
    let mut reverse = 0u128;
    for _ in 0..length {
        reverse = (reverse << 2) | ((code & 0b11) ^ 0b11);
        code >>= 2;
    }
    reverse
}

fn canonical_code(code: u128, length: u8) -> u128 {
    code.min(reverse_complement_code(code, length))
}

fn encode(sequence: &[u8]) -> u128 {
    sequence.iter().fold(0u128, |code, base| {
        (code << 2)
            | match base {
                b'A' => 0,
                b'C' => 1,
                b'G' => 2,
                b'T' => 3,
                _ => panic!("test sequence must contain only ACGT"),
            }
    })
}

fn canonical_link(link: GraphLink) -> GraphLink {
    let reverse = GraphLink {
        from_segment: link.to_segment.clone(),
        from_orientation: if link.to_orientation == '+' { '-' } else { '+' },
        to_segment: link.from_segment.clone(),
        to_orientation: if link.from_orientation == '+' {
            '-'
        } else {
            '+'
        },
    };
    let rank = |orientation| u8::from(orientation != '+');
    let forward_key = (
        link.from_segment.as_str(),
        rank(link.from_orientation),
        link.to_segment.as_str(),
        rank(link.to_orientation),
    );
    let reverse_key = (
        reverse.from_segment.as_str(),
        rank(reverse.from_orientation),
        reverse.to_segment.as_str(),
        rank(reverse.to_orientation),
    );
    if forward_key <= reverse_key {
        link
    } else {
        reverse
    }
}

fn verify_with_independent_oracle(k: u8, keys: &[u128]) {
    let records = keys
        .iter()
        .enumerate()
        .map(|(index, &key)| KmerCount {
            key,
            support: index as u64 + 1,
        })
        .collect::<Vec<_>>();
    let graph = ExactGraph::from_sorted_retained(k, &records, TEST_MEMORY_BUDGET).unwrap();
    let result = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
    let overlap = usize::from(k - 1);
    let node_mask = (1u128 << (2 * u32::from(k - 1))) - 1;

    let mut literal_handles = BTreeSet::new();
    for &key in keys {
        literal_handles.insert(key);
        literal_handles.insert(reverse_complement_code(key, k));
    }
    let is_boundary = |node: u128| {
        let incoming = literal_handles
            .iter()
            .filter(|&&handle| handle & node_mask == node)
            .count();
        let outgoing = literal_handles
            .iter()
            .filter(|&&handle| handle >> 2 == node)
            .count();
        let incident_self_reverse_complement = literal_handles.iter().any(|&handle| {
            handle == reverse_complement_code(handle, k)
                && (handle >> 2 == node || handle & node_mask == node)
        });
        incoming != 1
            || outgoing != 1
            || node == reverse_complement_code(node, k - 1)
            || incident_self_reverse_complement
    };
    let support_by_key = records
        .iter()
        .map(|record| (record.key, record.support))
        .collect::<BTreeMap<_, _>>();
    let mut owners = BTreeMap::<u128, usize>::new();

    for (unitig_index, unitig) in result.unitigs.iter().enumerate() {
        assert_eq!(
            unitig.sequence.len(),
            unitig.edge_steps as usize + overlap,
            "keys={keys:?}"
        );
        if unitig.topology == Topology::Linear {
            assert!(
                unitig.sequence <= oriented(&unitig.sequence, '-'),
                "linear representative is not RC-canonical; keys={keys:?}"
            );
        } else {
            let edge_steps = unitig.edge_steps as usize;
            let core = &unitig.sequence[..edge_steps];
            let reverse_core = oriented(core, '-');
            let mut candidates = Vec::new();
            for source in [core, reverse_core.as_slice()] {
                for shift in 0..edge_steps {
                    let mut rotated = source[shift..]
                        .iter()
                        .chain(&source[..shift])
                        .copied()
                        .collect::<Vec<_>>();
                    let closing = (0..overlap)
                        .map(|index| rotated[index % edge_steps])
                        .collect::<Vec<_>>();
                    rotated.extend_from_slice(&closing);
                    candidates.push(rotated);
                }
            }
            assert_eq!(
                &unitig.sequence,
                candidates.iter().min().unwrap(),
                "closed representative is not rotation/RC-canonical; keys={keys:?}"
            );
        }

        let steps = unitig
            .sequence
            .windows(usize::from(k))
            .map(encode)
            .collect::<Vec<_>>();
        assert!(
            steps.iter().all(|step| literal_handles.contains(step)),
            "unitig contains a non-graph step; keys={keys:?}"
        );
        for pair in steps.windows(2) {
            assert_eq!(pair[0] & node_mask, pair[1] >> 2, "keys={keys:?}");
        }
        let source = steps[0] >> 2;
        let target = steps[steps.len() - 1] & node_mask;
        match unitig.topology {
            Topology::Linear => {
                assert!(is_boundary(source), "nonmaximal source; keys={keys:?}");
                assert!(is_boundary(target), "nonmaximal target; keys={keys:?}");
                for step in &steps[..steps.len() - 1] {
                    assert!(
                        !is_boundary(step & node_mask),
                        "linear walk crosses a boundary; keys={keys:?}"
                    );
                }
            }
            Topology::ClosedGraphWalk => {
                assert_eq!(source, target, "open closed walk; keys={keys:?}");
                for step in &steps {
                    assert!(
                        !is_boundary(step >> 2),
                        "closed walk contains a boundary; keys={keys:?}"
                    );
                }
            }
        }

        let represented = unitig
            .sequence
            .windows(usize::from(k))
            .map(|window| canonical_code(encode(window), k))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            steps.len(),
            represented.len(),
            "unitig repeats a backing canonical key; keys={keys:?}"
        );
        assert_eq!(
            represented.len() as u64,
            unitig.canonical_kmers,
            "keys={keys:?}"
        );
        assert_eq!(unitig.edge_steps, unitig.canonical_kmers, "keys={keys:?}");
        let mut supports = represented
            .iter()
            .map(|key| support_by_key[key])
            .collect::<Vec<_>>();
        supports.sort_unstable();
        assert_eq!(unitig.minimum_support, supports[0], "keys={keys:?}");
        assert_eq!(
            unitig.lower_median_support,
            supports[(supports.len() - 1) / 2],
            "keys={keys:?}"
        );
        assert_eq!(
            unitig.maximum_support,
            supports[supports.len() - 1],
            "keys={keys:?}"
        );
        for key in represented {
            assert!(support_by_key.contains_key(&key), "keys={keys:?}");
            if let Some(previous) = owners.insert(key, unitig_index) {
                assert_eq!(previous, unitig_index, "keys={keys:?}");
            }
        }
    }
    assert_eq!(
        owners.keys().copied().collect::<BTreeSet<_>>(),
        support_by_key.keys().copied().collect(),
        "canonical-key ownership is incomplete; keys={keys:?}"
    );

    let mut expected_links = BTreeSet::new();
    for from in result
        .unitigs
        .iter()
        .filter(|unitig| unitig.topology == Topology::Linear)
    {
        for to in result
            .unitigs
            .iter()
            .filter(|unitig| unitig.topology == Topology::Linear)
        {
            for from_orientation in ['+', '-'] {
                let from_sequence = oriented(&from.sequence, from_orientation);
                for to_orientation in ['+', '-'] {
                    let to_sequence = oriented(&to.sequence, to_orientation);
                    if from_sequence[from_sequence.len() - overlap..] == to_sequence[..overlap] {
                        expected_links.insert(canonical_link(GraphLink {
                            from_segment: from.id.clone(),
                            from_orientation,
                            to_segment: to.id.clone(),
                            to_orientation,
                        }));
                    }
                }
            }
        }
    }
    for unitig in result
        .unitigs
        .iter()
        .filter(|unitig| unitig.topology == Topology::ClosedGraphWalk)
    {
        expected_links.insert(canonical_link(GraphLink {
            from_segment: unitig.id.clone(),
            from_orientation: '+',
            to_segment: unitig.id.clone(),
            to_orientation: '+',
        }));
    }
    let actual_links = result.links.iter().cloned().collect::<BTreeSet<_>>();
    assert_eq!(actual_links, expected_links, "keys={keys:?}");
}

#[test]
fn bounded_exhaustive_k3_oracle_checks_maximality_ownership_cycles_and_links() {
    let keys = (0..64u128)
        .map(|spelling| canonical_code(spelling, 3))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    verify_with_independent_oracle(3, &[]);
    verify_with_independent_oracle(3, &keys);
    for left in 0..keys.len() {
        verify_with_independent_oracle(3, &[keys[left]]);
        for middle in left + 1..keys.len() {
            verify_with_independent_oracle(3, &[keys[left], keys[middle]]);
            for right in middle + 1..keys.len() {
                verify_with_independent_oracle(3, &[keys[left], keys[middle], keys[right]]);
            }
        }
    }
}

#[test]
fn bounded_exhaustive_k4_oracle_covers_self_reverse_complement_edges() {
    let keys = (0..(1u128 << 8))
        .map(|spelling| canonical_code(spelling, 4))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let self_reverse_complement_keys = keys
        .iter()
        .copied()
        .filter(|key| *key == reverse_complement_code(*key, 4))
        .collect::<BTreeSet<_>>();
    assert_eq!(self_reverse_complement_keys.len(), 16);

    verify_with_independent_oracle(4, &[]);
    for left in 0..keys.len() {
        verify_with_independent_oracle(4, &[keys[left]]);
        for right in left + 1..keys.len() {
            verify_with_independent_oracle(4, &[keys[left], keys[right]]);
        }
    }

    // Exhaust every six-base DNA spelling whose three consecutive k=4
    // windows include a self-reverse-complement key. This covers both sides
    // of the fixed edge and bounded three-key contexts derived from an actual
    // input string, rather than only arbitrary key pairs.
    let kmer_mask = (1u128 << 8) - 1;
    for spelling in 0..(1u128 << 12) {
        let mut fixture = BTreeSet::new();
        let mut contains_fixed_edge = false;
        for offset in 0..=2u32 {
            let window = (spelling >> (2 * (2 - offset))) & kmer_mask;
            contains_fixed_edge |= window == reverse_complement_code(window, 4);
            fixture.insert(canonical_code(window, 4));
        }
        if contains_fixed_edge {
            verify_with_independent_oracle(4, &fixture.into_iter().collect::<Vec<_>>());
        }
    }
}

proptest! {
    #[test]
    fn exact_k4_subgraphs_respect_self_reverse_complement_boundaries(
        spellings in proptest::collection::vec(0u16..256, 0..48)
    ) {
        let keys = spellings
            .into_iter()
            .map(|spelling| canonical_code(u128::from(spelling), 4))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        verify_with_independent_oracle(4, &keys);
    }

    #[test]
    fn exact_k3_subgraphs_are_deterministic_and_conservative(
        spellings in proptest::collection::vec(0u8..64, 0..24)
    ) {
        let mut records = spellings
            .into_iter()
            .map(|spelling| KmerCount {
                key: canonical_code(u128::from(spelling), 3),
                support: 1 + u64::from(spelling % 11),
            })
            .collect::<Vec<_>>();
        records.sort_unstable_by_key(|record| record.key);
        records.dedup_by_key(|record| record.key);

        let graph = ExactGraph::from_sorted_retained(3, &records, TEST_MEMORY_BUDGET).unwrap();
        let first = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
        let second = compact_graph(&graph, TEST_MEMORY_BUDGET).unwrap();
        prop_assert_eq!(&first, &second);
        prop_assert_eq!(
            graph.stats().oriented_handles,
            2 * graph.stats().canonical_kmers - graph.stats().self_reverse_complement_keys
        );
        prop_assert_eq!(
            first.unitigs.iter().map(|unitig| unitig.canonical_kmers).sum::<u64>(),
            graph.stats().canonical_kmers
        );

        let identities = first
            .unitigs
            .iter()
            .map(|unitig| unitig.id.as_str())
            .collect::<BTreeSet<_>>();
        prop_assert_eq!(identities.len(), first.unitigs.len());

        for link in &first.links {
            let from = first
                .unitigs
                .iter()
                .find(|unitig| unitig.id == link.from_segment)
                .unwrap();
            let to = first
                .unitigs
                .iter()
                .find(|unitig| unitig.id == link.to_segment)
                .unwrap();
            let from_sequence = oriented(&from.sequence, link.from_orientation);
            let to_sequence = oriented(&to.sequence, link.to_orientation);
            prop_assert_eq!(
                &from_sequence[from_sequence.len() - 2..],
                &to_sequence[..2]
            );
        }
    }
}

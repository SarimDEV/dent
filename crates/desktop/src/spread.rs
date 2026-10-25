//! How a setting stands across machines, in few words however many there
//! are: the same everywhere, or Mixed, grouped by value, the odd ones out
//! by name and the rest counted (decision 0076).

/// One value and the machines that have it, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub value: String,
    pub names: Vec<String>,
}

/// How a setting stands across machines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spread {
    /// No machine said.
    Unknown,
    /// One value on every machine that said.
    Same { value: String, count: usize },
    /// More than one: the groups, most machines last.
    Mixed(Vec<Group>),
}

/// Names shown in a group before "and N more".
const NAMED: usize = 5;

/// How `values` (each machine's name and its value) stand.
pub fn spread(values: &[(String, String)]) -> Spread {
    if values.is_empty() {
        return Spread::Unknown;
    }
    let mut groups: Vec<Group> = Vec::new();
    for (name, value) in values {
        match groups.iter_mut().find(|g| g.value == *value) {
            Some(g) => g.names.push(name.clone()),
            None => groups.push(Group {
                value: value.clone(),
                names: vec![name.clone()],
            }),
        }
    }
    if groups.len() == 1 {
        let g = groups.remove(0);
        return Spread::Same {
            value: g.value,
            count: g.names.len(),
        };
    }
    // The odd ones first, by name; the most common last, counted.
    groups.sort_by(|a, b| {
        a.names
            .len()
            .cmp(&b.names.len())
            .then(a.value.cmp(&b.value))
    });
    Spread::Mixed(groups)
}

impl Spread {
    /// The line under a setting whose value shows beside it: "On all 4",
    /// "3 of 20 differ".
    pub fn brief(&self) -> String {
        match self {
            Spread::Unknown => "Not known yet".into(),
            Spread::Same { count: 1, .. } => String::new(),
            Spread::Same { count, .. } => format!("On all {count}"),
            Spread::Mixed(groups) => {
                let all: usize = groups.iter().map(|g| g.names.len()).sum();
                let most = groups.last().map_or(0, |g| g.names.len());
                // Two as common as each other: none is the odd one out.
                let tied = groups.len() > 1 && groups[groups.len() - 2].names.len() == most;
                if tied {
                    format!("Not the same on all {all}")
                } else {
                    format!("{} of {all} differ", all - most)
                }
            }
        }
    }

    /// Under a Mixed one, a line per value: the machines by name, the
    /// most common counted when there are many.
    pub fn lines(&self) -> Vec<(String, String)> {
        let Spread::Mixed(groups) = self else {
            return Vec::new();
        };
        let n = groups.len();
        groups
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let last = i + 1 == n;
                let who = if last && n > 1 && g.names.len() > NAMED {
                    format!("the other {}", g.names.len())
                } else {
                    named(&g.names)
                };
                (g.value.clone(), who)
            })
            .collect()
    }

    /// The value most machines have, for "Make all …".
    pub fn most(&self) -> Option<&str> {
        match self {
            Spread::Same { value, .. } => Some(value),
            Spread::Mixed(groups) => groups.last().map(|g| g.value.as_str()),
            Spread::Unknown => None,
        }
    }
}

/// "gpu-box, gpu-2 and build-box"; past a few, "…, and 4 more".
fn named(names: &[String]) -> String {
    let short: Vec<String> = names
        .iter()
        .map(|n| crate::data::machine_short(n))
        .collect();
    match short.len() {
        0 => String::new(),
        1 => short[0].clone(),
        n if n <= NAMED => format!("{} and {}", short[..n - 1].join(", "), short[n - 1]),
        n => format!("{} and {} more", short[..NAMED].join(", "), n - NAMED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn beside_its_value_it_says_only_how_many() {
        assert_eq!(
            spread(&of(&[("a", "Full"), ("b", "Full")])).brief(),
            "On all 2"
        );
        assert_eq!(
            spread(&of(&[("a", "Full"), ("b", "Ask"), ("c", "Full")])).brief(),
            "1 of 3 differ"
        );
        assert_eq!(spread(&of(&[("a", "Full")])).brief(), "");
    }

    #[test]
    fn one_value_everywhere_is_said_once() {
        let s = spread(&of(&[("a", "Full"), ("b", "Full"), ("c", "Full")]));
        assert_eq!(s.brief(), "On all 3");
        assert!(s.lines().is_empty());
        assert_eq!(spread(&of(&[("mac", "Full")])).brief(), "");
        assert_eq!(spread(&[]).brief(), "Not known yet");
    }

    /// Twenty machines, three differ: the three by name, the rest counted.
    #[test]
    fn many_machines_name_the_odd_ones_and_count_the_rest() {
        let mut v: Vec<(String, String)> =
            (0..17).map(|i| (format!("m{i}"), "Full".into())).collect();
        for n in ["gpu-box", "gpu-2", "build-box"] {
            v.push((n.into(), "Ask".into()));
        }
        let s = spread(&v);
        assert_eq!(s.brief(), "3 of 20 differ");
        assert_eq!(
            s.lines(),
            [
                (
                    "Ask".to_string(),
                    "gpu-box, gpu-2 and build-box".to_string()
                ),
                ("Full".to_string(), "the other 17".to_string()),
            ]
        );
        assert_eq!(s.most(), Some("Full"));
    }

    /// Nine on one side, eleven on the other: the smaller cut short, the
    /// larger counted; one machine apart says "1 differs".
    #[test]
    fn big_groups_are_cut_short() {
        let mut v: Vec<(String, String)> =
            (0..9).map(|i| (format!("ci-{i}"), "On".into())).collect();
        v.extend((0..11).map(|i| (format!("m{i}"), "Off".into())));
        let s = spread(&v);
        assert_eq!(s.brief(), "9 of 20 differ");
        assert_eq!(s.lines()[0].1, "ci-0, ci-1, ci-2, ci-3, ci-4 and 4 more");
        assert_eq!(s.lines()[1].1, "the other 11");
        // One each: neither is the odd one out.
        let s = spread(&of(&[("a", "Full"), ("b", "Ask")]));
        assert_eq!(s.brief(), "Not the same on all 2");
        // Few: everyone named, no "the other".
        assert_eq!(s.lines()[1].1, "a");
        let long = spread(&of(&[
            ("research-gpu-cluster-node-07", "Ask"),
            ("b", "Full"),
            ("c", "Full"),
        ]));
        assert_eq!(long.lines()[0].1, "research-gpu-c…-node-07");
    }
}

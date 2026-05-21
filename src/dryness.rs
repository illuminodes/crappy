use std::collections::{HashMap, HashSet};

use crate::idiom::FunctionIdioms;

const DRY_WEIGHT: u32 = 3;

pub fn check_dryness(functions: &mut [FunctionIdioms]) {
    let sig_flagged = find_duplicate_indices(functions, |f| f.sig_fingerprint.as_str());
    let body_flagged = find_duplicate_indices(functions, |f| f.body_fingerprint.as_str());

    for (i, func) in functions.iter_mut().enumerate() {
        if sig_flagged.contains(&i) {
            func.demerits += DRY_WEIGHT;
        }
        if body_flagged.contains(&i) {
            func.demerits += DRY_WEIGHT;
        }
    }
}

fn find_duplicate_indices(
    functions: &[FunctionIdioms],
    key_fn: fn(&FunctionIdioms) -> &str,
) -> HashSet<usize> {
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();

    for (i, func) in functions.iter().enumerate() {
        let key = key_fn(func);
        if !key.is_empty() {
            groups.entry(key).or_default().push(i);
        }
    }

    let mut flagged = HashSet::new();
    for indices in groups.values() {
        if indices.len() >= 2 {
            for &i in indices {
                flagged.insert(i);
            }
        }
    }

    flagged
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn func(name: &str, sig: &str, body: &str) -> FunctionIdioms {
        FunctionIdioms {
            file: PathBuf::from("test.rs"),
            qualified_name: name.to_string(),
            demerits: 0,
            sig_fingerprint: sig.to_string(),
            body_fingerprint: body.to_string(),
        }
    }

    #[test]
    fn no_duplicates() {
        let mut fns = vec![
            func("a", "i32->bool", "body_a"),
            func("b", "String->u32", "body_b"),
        ];
        check_dryness(&mut fns);
        assert_eq!(fns[0].demerits, 0);
        assert_eq!(fns[1].demerits, 0);
    }

    #[test]
    fn signature_duplicates() {
        let mut fns = vec![
            func("a", "i32->bool", "body_a"),
            func("b", "i32->bool", "body_b"),
        ];
        check_dryness(&mut fns);
        assert_eq!(fns[0].demerits, DRY_WEIGHT);
        assert_eq!(fns[1].demerits, DRY_WEIGHT);
    }

    #[test]
    fn body_duplicates() {
        let mut fns = vec![
            func("a", "i32->bool", "same_body"),
            func("b", "String->u32", "same_body"),
        ];
        check_dryness(&mut fns);
        assert_eq!(fns[0].demerits, DRY_WEIGHT);
        assert_eq!(fns[1].demerits, DRY_WEIGHT);
    }

    #[test]
    fn both_sig_and_body_duplicate_stacks() {
        let mut fns = vec![
            func("a", "i32->bool", "same_body"),
            func("b", "i32->bool", "same_body"),
        ];
        check_dryness(&mut fns);
        assert_eq!(fns[0].demerits, DRY_WEIGHT * 2);
        assert_eq!(fns[1].demerits, DRY_WEIGHT * 2);
    }

    #[test]
    fn three_way_duplicate() {
        let mut fns = vec![
            func("a", "i32->bool", "x"),
            func("b", "i32->bool", "y"),
            func("c", "i32->bool", "z"),
        ];
        check_dryness(&mut fns);
        for f in &fns {
            assert_eq!(f.demerits, DRY_WEIGHT);
        }
    }

    #[test]
    fn empty_fingerprints_ignored() {
        let mut fns = vec![func("a", "", ""), func("b", "", "")];
        check_dryness(&mut fns);
        assert_eq!(fns[0].demerits, 0);
        assert_eq!(fns[1].demerits, 0);
    }

    #[test]
    fn preserves_existing_demerits() {
        let mut fns = vec![
            FunctionIdioms {
                file: PathBuf::from("test.rs"),
                qualified_name: "a".to_string(),
                demerits: 5,
                sig_fingerprint: "i32->bool".to_string(),
                body_fingerprint: "unique".to_string(),
            },
            func("b", "i32->bool", "other"),
        ];
        check_dryness(&mut fns);
        assert_eq!(fns[0].demerits, 5 + DRY_WEIGHT);
    }
}

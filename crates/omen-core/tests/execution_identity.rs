use std::collections::HashSet;

use omen_core::ExecutionId;

#[test]
fn generated_execution_ids_are_opaque_and_unique() {
    let ids: Vec<_> = (0..256).map(|_| ExecutionId::generate()).collect();
    let distinct: HashSet<_> = ids.iter().map(|id| id.as_str()).collect();

    assert_eq!(distinct.len(), ids.len());
    assert!(ids.iter().all(|id| id.as_str().starts_with("exec_")));
    assert!(ids.iter().all(|id| !id.as_str().contains(' ')));
}

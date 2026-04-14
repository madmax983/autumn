sed -i 's/let mut config = AutumnConfig::default();/#[allow(clippy::field_reassign_with_default)]\n    let mut config = AutumnConfig::default();/g' autumn/tests/probe_contracts.rs

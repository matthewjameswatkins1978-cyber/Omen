//! Pure kit tests: manifest, lifecycle, negotiation, env policy,
//! wire bounds, binding fingerprints. No processes, no network.

use omen_agent_adapter::protocol::*;
use omen_agent_adapter::*;

// ---------- helpers ----------

fn good_manifest() -> AdapterManifest {
    AdapterManifest {
        schema_version: 1,
        adapter_id: "codex".into(),
        adapter_version: "0.1.0".into(),
        entrypoint: vec!["codex".into(), "exec".into()],
        supported_protocol_versions: vec!["0.1".into()],
        required_omen_contracts: vec!["0.8".into()],
        capabilities: vec!["reasoning".into()],
        required_programs: vec!["codex".into()],
        required_config_labels: vec!["codex-auth".into()],
        credential_labels: vec!["codex-auth".into()],
        transports: vec!["codex-exec".into()],
        streaming: false,
        platforms: vec!["windows".into(), "macos".into(), "linux".into()],
        optional_features: vec!["streaming".into()],
        required_features: vec![],
        extra: Default::default(),
    }
}

// ---------- manifest ----------

#[test]
fn manifest_good_validates() {
    good_manifest().validate().unwrap();
}

#[test]
fn manifest_rejects_shell_entrypoint() {
    for bad in [
        vec!["sh -c exec".to_string()],
        vec!["codex | tee".to_string()],
        vec!["run $(evil)".to_string()],
        vec!["a; b".to_string()],
        vec!["x && y".to_string()],
    ] {
        let mut m = good_manifest();
        m.entrypoint = bad;
        assert!(m.validate().is_err(), "shell entrypoint must be rejected");
    }
}

#[test]
fn manifest_rejects_authority_shaped_fields() {
    for key in [
        "permissions",
        "allow_filesystem",
        "grant_admin",
        "authority",
        "api_key",
        "bearer_token",
        "secret",
    ] {
        let mut m = good_manifest();
        m.extra.insert(key.into(), serde_json::json!(true));
        assert!(
            m.validate().is_err(),
            "authority/secret shaped field {key:?} must be rejected"
        );
    }
}

#[test]
fn manifest_rejects_secret_value_as_label() {
    let mut m = good_manifest();
    m.credential_labels = vec!["sk-".to_string() + &"x".repeat(200)];
    assert_eq!(
        m.validate(),
        Err(manifest::ManifestError::SecretValueInManifest)
    );
}

#[test]
fn manifest_requirements_are_not_permission() {
    // The manifest type has no field capable of expressing a grant:
    // requirements are need-declarations, admission stays authoritative.
    let m = good_manifest();
    let json = serde_json::to_value(&m).unwrap();
    for key in [
        "permissions",
        "allow",
        "grant",
        "authority",
        "token",
        "secret",
    ] {
        assert!(json.get(key).is_none(), "manifest must not carry {key:?}");
    }
    m.validate().unwrap();
}

#[test]
fn manifest_unknown_fields_preserved_but_inert() {
    let mut m = good_manifest();
    m.extra
        .insert("vendor_note".into(), serde_json::json!("hello"));
    m.validate().unwrap();
    // Unknown metadata round-trips but changes nothing validated.
    let json = serde_json::to_value(&m).unwrap();
    assert_eq!(json["vendor_note"], serde_json::json!("hello"));
    assert_eq!(json["adapter_id"], serde_json::json!("codex"));
}

// ---------- lifecycle ----------

#[test]
fn lifecycle_happy_path() {
    use AdapterLifecycle as S;
    use LifecycleEvent as E;
    let s = S::Inspected;
    let s = s.advance(E::ManifestValid).unwrap();
    let s = s.advance(E::ConformancePass).unwrap();
    let s = s.advance(E::Install).unwrap();
    let s = s.advance(E::Bind).unwrap();
    let s = s.advance(E::Activate).unwrap();
    assert_eq!(s, S::Active);
    assert!(s.is_live());
}

#[test]
fn lifecycle_conformance_does_not_install_or_activate() {
    use AdapterLifecycle as S;
    use LifecycleEvent as E;
    let s = S::Validated.advance(E::ConformancePass).unwrap();
    assert_eq!(s, S::Conformed);
    assert!(!s.is_live());
    // No shortcut exists: Conformed + Activate is illegal, must Install+Bind first.
    assert!(s.advance(E::Activate).is_err());
    assert!(s.advance(E::Bind).is_err());
}

#[test]
fn lifecycle_illegal_transitions_fail_visibly() {
    use AdapterLifecycle as S;
    use LifecycleEvent as E;
    assert!(S::Inspected.advance(E::Activate).is_err());
    assert!(S::Active.advance(E::Install).is_err());
    assert!(S::Disabled.advance(E::Bind).is_err());
    assert!(S::Quarantined.advance(E::Activate).is_err());
}

#[test]
fn lifecycle_side_states_preserve_distinctions() {
    use AdapterLifecycle as S;
    use LifecycleEvent as E;
    assert_eq!(S::Active.advance(E::Degrade).unwrap(), S::Degraded);
    assert_eq!(S::Degraded.advance(E::Recover).unwrap(), S::Active);
    assert_eq!(
        S::Active.advance(E::DependencyGone).unwrap(),
        S::Unavailable
    );
    assert_eq!(S::Active.advance(E::Quarantine).unwrap(), S::Quarantined);
    assert_eq!(S::Quarantined.advance(E::Release).unwrap(), S::Inspected);
    assert_eq!(S::Installed.advance(E::MarkStale).unwrap(), S::Stale);
}

// ---------- negotiation ----------

fn omen_offer() -> OmenOffer {
    OmenOffer {
        omen_contract: "0.8".into(),
        adapter_protocols: vec!["0.1".into()],
        required_features: vec![],
        optional_features: vec!["streaming".into()],
    }
}

fn adapter_offer() -> AdapterOffer {
    AdapterOffer {
        protocol_version: "0.1".into(),
        features: vec![],
        contract_versions: vec!["0.8".into()],
    }
}

#[test]
fn negotiate_happy_with_optional_degraded() {
    let n = negotiate(&omen_offer(), &adapter_offer()).unwrap();
    assert_eq!(n.protocol_version, "0.1");
    assert_eq!(n.degraded_features, vec!["streaming".to_string()]);
    assert!(n.active_features.is_empty());
}

#[test]
fn negotiate_optional_present_activates() {
    let mut adapter = adapter_offer();
    adapter.features = vec!["streaming".into()];
    let n = negotiate(&omen_offer(), &adapter).unwrap();
    assert_eq!(n.active_features, vec!["streaming".to_string()]);
    assert!(n.degraded_features.is_empty());
}

#[test]
fn negotiate_refuses_incompatible_protocol() {
    let mut adapter = adapter_offer();
    adapter.protocol_version = "9.9".into();
    let r = negotiate(&omen_offer(), &adapter).unwrap_err();
    assert_eq!(r.reason, NegotiationRefusalReason::IncompatibleProtocol);
}

#[test]
fn negotiate_refuses_incompatible_contract() {
    let mut adapter = adapter_offer();
    adapter.contract_versions = vec!["0.7".into()];
    let r = negotiate(&omen_offer(), &adapter).unwrap_err();
    assert_eq!(r.reason, NegotiationRefusalReason::IncompatibleContract);
}

#[test]
fn negotiate_refuses_missing_required_feature() {
    let mut omen = omen_offer();
    omen.required_features = vec!["tools".into()];
    let r = negotiate(&omen, &adapter_offer()).unwrap_err();
    assert_eq!(r.reason, NegotiationRefusalReason::MissingRequiredFeature);
}

// ---------- env policy ----------

#[test]
fn env_policy_drops_everything_unlisted() {
    let policy = codex_env_policy();
    let parent = vec![
        ("OPENAI_API_KEY".into(), "sk-live-SYNTHETIC".into()),
        ("OMEN_SYNTHETIC_SECRET".into(), "shhh-SYNTHETIC".into()),
        ("SystemRoot".into(), "C:\\Windows".into()),
        ("RANDOM_JUNK".into(), "1".into()),
    ];
    let env = policy.build_env(&parent, &["codex-auth".into()]);
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert!(keys.contains(&"SystemRoot"));
    assert!(!keys.contains(&"OPENAI_API_KEY"));
    assert!(!keys.contains(&"OMEN_SYNTHETIC_SECRET"));
    assert!(!keys.contains(&"RANDOM_JUNK"));
    EnvPolicy::assert_no_secret_leak(&env, &["sk-live-SYNTHETIC", "shhh-SYNTHETIC"]).unwrap();
}

#[test]
fn env_policy_sets_non_secret_literals() {
    let policy = codex_env_policy();
    let env = policy.build_env(&[], &[]);
    let no_color = env.iter().find(|(k, _)| k == "NO_COLOR").unwrap();
    assert_eq!(no_color.1, "1");
}

#[test]
fn env_policy_audit_catches_leak() {
    let env = vec![("X".into(), "prefix-sk-live-SYNTHETIC-suffix".into())];
    assert!(EnvPolicy::assert_no_secret_leak(&env, &["sk-live-SYNTHETIC"]).is_err());
}

// ---------- wire bounds ----------

fn hello_frame() -> AdapterHello {
    AdapterHello {
        protocol: format!("{ADAPTER_PROTOCOL_SCHEMA}/{ADAPTER_PROTOCOL_VERSION}"),
        adapter_id: "t".into(),
        adapter_version: "0.1.0".into(),
        protocol_version: "0.1".into(),
        capabilities: vec![],
        features: vec![],
        contract_versions: vec!["0.8".into()],
        credential_labels: vec![],
        extra: Default::default(),
    }
}

#[test]
fn wire_round_trip_hello() {
    let frame = AdapterFrame::Hello {
        id: 1,
        payload: hello_frame(),
    };
    let line = encode_frame(&frame).unwrap();
    assert_eq!(decode_adapter_frame(&line).unwrap(), frame);
}

#[test]
fn wire_rejects_wrong_protocol() {
    let mut value = serde_json::to_value(&AdapterFrame::Hello {
        id: 1,
        payload: hello_frame(),
    })
    .unwrap();
    value["protocol"] = serde_json::json!("omen.agent-adapter/9.9");
    let line = serde_json::to_string(&value).unwrap();
    assert!(matches!(
        decode_adapter_frame(&line),
        Err(ProtocolError::WrongProtocol { .. })
    ));
}

#[test]
fn wire_rejects_missing_protocol() {
    let line = serde_json::to_string(&AdapterFrame::Hello {
        id: 1,
        payload: hello_frame(),
    })
    .unwrap();
    assert!(decode_adapter_frame(&line).is_err());
}

#[test]
fn wire_rejects_malformed_and_empty() {
    assert!(decode_adapter_frame("not json {{{").is_err());
    assert!(decode_adapter_frame("").is_err());
    assert!(decode_adapter_frame("   ").is_err());
}

#[test]
fn wire_rejects_oversize_frame() {
    let big = "x".repeat(MAX_FRAME_BYTES + 16);
    assert!(matches!(
        decode_adapter_frame(&big),
        Err(ProtocolError::FrameTooLarge { .. })
    ));
}

#[test]
fn wire_rejects_too_many_actions() {
    let frame = AdapterFrame::Response {
        id: 1,
        payload: AdapterResponse {
            request_id: "r".into(),
            kind: "proposal".into(),
            message: "m".into(),
            proposed_actions: vec![serde_json::json!({}); MAX_ACTIONS_PER_RESPONSE + 1],
            references: vec![],
            uncertainty: None,
            extra: Default::default(),
        },
    };
    let line = encode_frame(&frame).unwrap();
    assert!(matches!(
        decode_adapter_frame(&line),
        Err(ProtocolError::TooManyActions { .. })
    ));
}

#[test]
fn wire_unknown_fields_preserved_and_bounded() {
    let mut hello = hello_frame();
    hello
        .extra
        .insert("vendor".into(), serde_json::json!({"a": 1}));
    let frame = AdapterFrame::Hello {
        id: 1,
        payload: hello,
    };
    let line = encode_frame(&frame).unwrap();
    match decode_adapter_frame(&line).unwrap() {
        AdapterFrame::Hello { payload, .. } => {
            assert_eq!(payload.extra["vendor"], serde_json::json!({"a": 1}));
            assert_eq!(payload.adapter_id, "t");
        }
        _ => panic!("expected hello"),
    }
    // ...but too many unknown fields fail rather than growing memory.
    let mut hello2 = hello_frame();
    for i in 0..(MAX_UNKNOWN_FIELDS + 5) {
        hello2.extra.insert(format!("u{i}"), serde_json::json!(i));
    }
    let line2 = encode_frame(&AdapterFrame::Hello {
        id: 1,
        payload: hello2,
    })
    .unwrap();
    assert!(matches!(
        decode_adapter_frame(&line2),
        Err(ProtocolError::TooManyUnknownFields { .. })
    ));
}

// ---------- binding ----------

#[test]
fn binding_fingerprint_is_deterministic_and_secret_free() {
    let a = fingerprint_pairs(&[("sandbox", "read-only"), ("argv0", "codex")]);
    let b = fingerprint_pairs(&[("argv0", "codex"), ("sandbox", "read-only")]);
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
    assert!(!a.contains("sk-"));
}

#[test]
fn binding_model_unknown_by_default() {
    let b = BindingIdentity {
        adapter_id: "codex".into(),
        adapter_version: "0.154.0".into(),
        adapter_digest: "abc".into(),
        manifest_digest: "def".into(),
        protocol_version: "codex-exec".into(),
        provider_id: "codex".into(),
        model: None,
        transport: "codex-exec".into(),
        omen_contract: "0.8".into(),
        config_fingerprint: "00".into(),
        bound_at: "now".into(),
    };
    assert_eq!(b.model_or_unknown(), "UNKNOWN");
}

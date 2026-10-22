// SPDX-License-Identifier: Apache-2.0
//! Source connectors and memory type helpers.
//!
//! The core memory types (`MemoryType`, `RawDocument`, `SourceType`,
//! `StabilityTier`, `SyncStatus`) live in `wenlan-types` and are re-exported
//! here so intra-crate imports keep working via `crate::sources::*`.
//!
//! This module owns helpers that need `tuning::ConfidenceConfig` (confidence
//! computation, decay rates).
pub mod directory;
pub mod docx;
pub mod logseq;
pub mod obsidian;
pub mod okf;
pub mod page_watcher;

// Re-export canonical type definitions from wenlan-types. This keeps
// `crate::sources::RawDocument` working for wenlan-core modules while the
// authoritative definition lives in the shared types crate.
pub use wenlan_types::sources::{
    stability_tier, MemoryType, RawDocument, Source, SourceStatus, SourceType, StabilityTier,
    SyncStatus,
};

/// Return the confidence ceiling for a stability tier.
pub fn base_confidence(tier: &StabilityTier, cfg: &crate::tuning::ConfidenceConfig) -> f32 {
    match tier {
        StabilityTier::Protected => cfg.protected_base,
        StabilityTier::Standard => cfg.standard_base,
        StabilityTier::Ephemeral => cfg.ephemeral_base,
    }
}

/// Return the trust weighting for a given agent trust level.
pub fn trust_weight(trust_level: &str, cfg: &crate::tuning::ConfidenceConfig) -> f32 {
    match trust_level {
        "full" => cfg.full_trust_weight,
        "review" => cfg.review_trust_weight,
        _ => cfg.untrusted_weight,
    }
}

/// Return the decay rate for a stability tier.
pub fn decay_rate(tier: &StabilityTier, cfg: &crate::tuning::ConfidenceConfig) -> f64 {
    match tier {
        StabilityTier::Protected => cfg.protected_decay,
        StabilityTier::Standard => cfg.standard_decay,
        StabilityTier::Ephemeral => cfg.ephemeral_decay,
    }
}

/// Compute effective confidence from agent-supplied value, memory type, trust level, and quality.
/// If agent provides a value, it's clamped to the max allowed by tier * trust.
/// If agent provides no value, uses the max as default.
/// If quality is "low", the result is multiplied by low_quality_multiplier (default 0.7).
pub fn compute_effective_confidence(
    agent_confidence: Option<f32>,
    memory_type: Option<&str>,
    trust_level: &str,
    quality: Option<&str>,
    cfg: &crate::tuning::ConfidenceConfig,
) -> f32 {
    let tier = stability_tier(memory_type);
    let max_confidence = base_confidence(&tier, cfg) * trust_weight(trust_level, cfg);
    let base = match agent_confidence {
        Some(val) => val.min(max_confidence),
        None => max_confidence,
    };
    if quality == Some("low") {
        base * cfg.low_quality_multiplier
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_memory_type_from_str_valid() {
        assert_eq!(
            "identity".parse::<MemoryType>().unwrap(),
            MemoryType::Identity
        );
        assert_eq!(
            "preference".parse::<MemoryType>().unwrap(),
            MemoryType::Preference
        );
        assert_eq!(
            "decision".parse::<MemoryType>().unwrap(),
            MemoryType::Decision
        );
        assert_eq!("lesson".parse::<MemoryType>().unwrap(), MemoryType::Lesson);
        assert_eq!("gotcha".parse::<MemoryType>().unwrap(), MemoryType::Gotcha);
        assert_eq!("fact".parse::<MemoryType>().unwrap(), MemoryType::Fact);
        // Deprecated: "goal" folds into Identity (aspirations = identity).
        assert_eq!("goal".parse::<MemoryType>().unwrap(), MemoryType::Identity);
        // Case insensitive
        assert_eq!(
            "IDENTITY".parse::<MemoryType>().unwrap(),
            MemoryType::Identity
        );
        assert_eq!("Fact".parse::<MemoryType>().unwrap(), MemoryType::Fact);
        assert_eq!("GOTCHA".parse::<MemoryType>().unwrap(), MemoryType::Gotcha);
    }

    #[test]
    fn test_memory_type_aliases_require_subclassification() {
        assert!("profile".parse::<MemoryType>().is_err());
        assert!("knowledge".parse::<MemoryType>().is_err());
    }

    #[test]
    fn test_stability_tier_protected() {
        assert!(matches!(
            stability_tier(Some("identity")),
            StabilityTier::Protected
        ));
        assert!(matches!(
            stability_tier(Some("preference")),
            StabilityTier::Protected
        ));
        // Deprecated rows: "goal" still maps to Protected via Identity fold.
        assert!(matches!(
            stability_tier(Some("goal")),
            StabilityTier::Protected
        ));
    }

    #[test]
    fn test_stability_tier_standard() {
        assert!(matches!(
            stability_tier(Some("fact")),
            StabilityTier::Standard
        ));
        assert!(matches!(
            stability_tier(Some("decision")),
            StabilityTier::Standard
        ));
        assert!(matches!(
            stability_tier(Some("lesson")),
            StabilityTier::Standard
        ));
        assert!(matches!(
            stability_tier(Some("gotcha")),
            StabilityTier::Standard
        ));
    }

    #[test]
    fn test_stability_tier_ephemeral() {
        assert!(matches!(stability_tier(None), StabilityTier::Ephemeral));
        assert!(matches!(
            stability_tier(Some("unknown_junk")),
            StabilityTier::Ephemeral
        ));
    }

    #[test]
    fn test_base_confidence_values() {
        let cfg = crate::tuning::ConfidenceConfig::default();
        assert_eq!(base_confidence(&StabilityTier::Protected, &cfg), 0.90);
        assert_eq!(base_confidence(&StabilityTier::Standard, &cfg), 0.70);
        assert_eq!(base_confidence(&StabilityTier::Ephemeral, &cfg), 0.50);
    }

    #[test]
    fn test_trust_weight_values() {
        let cfg = crate::tuning::ConfidenceConfig::default();
        assert_eq!(trust_weight("full", &cfg), 1.0);
        assert_eq!(trust_weight("review", &cfg), 0.7);
        assert_eq!(trust_weight("untrusted", &cfg), 0.4);
        assert_eq!(trust_weight("something_else", &cfg), 0.4);
    }

    #[test]
    fn test_compute_effective_confidence() {
        let cfg = crate::tuning::ConfidenceConfig::default();
        // Full trust + Protected = 0.90
        assert!(
            (compute_effective_confidence(None, Some("identity"), "full", None, &cfg) - 0.90).abs()
                < 0.001
        );
        // Review trust + Protected = 0.63
        assert!(
            (compute_effective_confidence(None, Some("identity"), "review", None, &cfg) - 0.63)
                .abs()
                < 0.001
        );
        // Agent provides lower value — kept
        assert!(
            (compute_effective_confidence(Some(0.5), Some("identity"), "full", None, &cfg) - 0.5)
                .abs()
                < 0.001
        );
        // Agent provides higher value — clamped
        assert!(
            (compute_effective_confidence(Some(0.99), Some("fact"), "review", None, &cfg) - 0.49)
                .abs()
                < 0.001
        );
        // NULL memory_type → Ephemeral base
        assert!(
            (compute_effective_confidence(None, None, "full", None, &cfg) - 0.50).abs() < 0.001
        );
    }

    #[test]
    fn test_compute_effective_confidence_quality_low() {
        let cfg = crate::tuning::ConfidenceConfig::default();
        // quality = "low" multiplies result by 0.7
        // Full trust + Protected = 0.90 * 0.7 = 0.63
        assert!(
            (compute_effective_confidence(None, Some("identity"), "full", Some("low"), &cfg)
                - 0.63)
                .abs()
                < 0.001
        );
        // Full trust + Standard = 0.70 * 0.7 = 0.49
        assert!(
            (compute_effective_confidence(None, Some("fact"), "full", Some("low"), &cfg) - 0.49)
                .abs()
                < 0.001
        );
    }

    #[test]
    fn test_compute_effective_confidence_quality_non_low() {
        let cfg = crate::tuning::ConfidenceConfig::default();
        // quality = "medium" or "high" should NOT reduce confidence
        let base = compute_effective_confidence(None, Some("fact"), "full", None, &cfg);
        let medium = compute_effective_confidence(None, Some("fact"), "full", Some("medium"), &cfg);
        let high = compute_effective_confidence(None, Some("fact"), "full", Some("high"), &cfg);
        assert!((base - medium).abs() < 0.001);
        assert!((base - high).abs() < 0.001);
    }

    #[test]
    fn test_decay_rate_values() {
        let cfg = crate::tuning::ConfidenceConfig::default();
        assert!((decay_rate(&StabilityTier::Protected, &cfg) - 0.001).abs() < 0.0001);
        assert!((decay_rate(&StabilityTier::Standard, &cfg) - 0.01).abs() < 0.001);
        assert!((decay_rate(&StabilityTier::Ephemeral, &cfg) - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_source_roundtrip() {
        let source = Source {
            id: "obsidian-main".to_string(),
            source_type: SourceType::Obsidian,
            path: PathBuf::from("/Users/x/vault"),
            status: SyncStatus::Active,
            last_sync: Some(1712678400),
            file_count: 42,
            memory_count: 128,
            last_sync_errors: 0,
            last_sync_error_detail: None,
            space: None,
            queued_files: 0,
            waiting_files: 0,
        };
        let json = serde_json::to_string(&source).unwrap();
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "obsidian-main");
        assert_eq!(back.source_type, SourceType::Obsidian);
        assert_eq!(back.file_count, 42);
    }

    #[test]
    fn test_source_without_okf_fields_loads_with_defaults() {
        // A config written before OKF import has no space or batch counts.
        let json = r#"{"id":"directory-notes","source_type":"directory","path":"/Users/x/notes","status":"Active","last_sync":null,"file_count":3,"memory_count":9}"#;
        let source: Source = serde_json::from_str(json).unwrap();
        assert_eq!(source.space, None);
        assert_eq!(source.queued_files, 0);
        assert_eq!(source.waiting_files, 0);
        let written = serde_json::to_string(&source).unwrap();
        assert!(
            !written.contains("\"space\""),
            "unset space is not written: {written}"
        );

        let okf = r#"{"id":"okf-wiki","source_type":"okf","path":"/Users/x/wiki","status":"Active","last_sync":null,"space":"work","queued_files":1000,"waiting_files":200}"#;
        let source: Source = serde_json::from_str(okf).unwrap();
        assert_eq!(source.source_type, SourceType::Okf);
        assert_eq!(source.space.as_deref(), Some("work"));
        assert_eq!((source.queued_files, source.waiting_files), (1000, 200));
    }

    #[test]
    fn test_source_type_display() {
        assert_eq!(SourceType::Obsidian.as_str(), "obsidian");
        assert_eq!(SourceType::Directory.as_str(), "directory");
        assert_eq!(SourceType::Okf.as_str(), "okf");
    }

    #[test]
    fn test_sync_status_variants() {
        let active = SyncStatus::Active;
        let err = SyncStatus::Error("bad path".to_string());
        let json_err = serde_json::to_string(&err).unwrap();
        assert!(json_err.contains("bad path"));
        let json_active = serde_json::to_string(&active).unwrap();
        assert!(json_active.contains("Active"));
    }
}

// SPDX-License-Identifier: Apache-2.0

//! Registry of on-device GGUF models Wenlan can download and run itself.
//!
//! The list spans several families and sizes, ordered by memory. Which ones a
//! computer is offered follows its hardware ([`model_fit`], [`shortlist`]). Qwen 3 4B
//! stays the default: it runs on any laptop. Only Qwen 3 4B and Qwen 3.5 9B
//! have been quality-measured on Wenlan's jobs; the others are offered as
//! choices and measured later.
//!
//! ## Token budget calibration (2026-04-18)
//!
//! These values are initial engineering estimates aligned with llama.cpp
//! community practice for Q4_K_M quantized models on Apple Silicon, not
//! benchmark-derived constants. Subject to revision as we collect empirical
//! data from real distillation runs.
//!
//! - `context_size`: KV cache allocation. Community norm is 4K-8K for sub-7B
//!   Q4 models, 8K-16K for 7B-13B. Constrained by Metal GPU memory, not
//!   model capability. DeltaNet (9B) handles longer contexts than standard
//!   Transformers at the same parameter count.
//! - `max_output_tokens`: quality-constrained, not context-constrained. Small
//!   quantized models lose coherence and start repeating past ~750-1000 words.
//!   9B sustains structured output (wiki format) up to ~1500 words.
//! - `synthesis_token_limit`: effective input capacity for synthesis. Set well
//!   below the trained context to account for quality degradation of quantized
//!   models at long contexts.

use std::path::PathBuf;

/// How prompts are written for a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFormat {
    /// Wenlan's own Qwen ChatML template (see `format_chatml_prompt`).
    ChatMl,
    /// The chat template stored inside the GGUF file, rendered by llama.cpp.
    /// Used for families whose turn markers are not ChatML (Gemma).
    ModelTemplate,
}

#[derive(Debug, Clone)]
pub struct OnDeviceModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub repo_id: &'static str,
    pub filename: &'static str,
    pub param_count: &'static str,
    pub ram_required_gb: f64,
    pub file_size_gb: f64,
    /// Max tokens this model can effectively synthesize (not just read).
    /// Research-calibrated: ~25% of context window for quantized models,
    /// accounting for quality degradation at longer contexts.
    pub synthesis_token_limit: usize,
    /// KV cache context window size for inference. Controls how many tokens
    /// (prompt + output) fit in one call. Larger = more GPU memory. Set per
    /// model to balance capacity vs memory pressure.
    pub context_size: u32,
    /// Recommended max output tokens for body-generation tasks (distillation,
    /// re-distillation). Smaller models produce thin/repetitive output at
    /// high token counts, so this is quality-constrained, not context-constrained.
    pub max_output_tokens: u32,
    /// True when the model starts every answer in thinking mode unless told
    /// not to. The prompt then opens with an empty `<think></think>` block so
    /// the whole output budget goes to the answer. Models without a thinking
    /// mode must NOT get that block: Qwen3-4B-Instruct-2507 reads it as an
    /// answer that already ended and, at the title job's sampling
    /// temperature, stops before its first word (greedy decoding answers).
    pub thinking_mode: bool,
    /// How prompts are written for this model.
    pub prompt_format: PromptFormat,
}

/// All available on-device models, ordered by memory needed.
pub const MODELS: &[OnDeviceModel] = &[
    OnDeviceModel {
        id: "qwen3-4b",
        display_name: "Qwen 3 4B",
        repo_id: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
        filename: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        param_count: "4B",
        ram_required_gb: 3.0,
        file_size_gb: 2.7,
        synthesis_token_limit: 6_000, // 32K context, ~19% — conservative for 4B quantized
        context_size: 8_192,          // 25% of 32K. KV cache ~2GB on Metal.
        max_output_tokens: 1_024,     // 4B quality degrades past ~750 words
        thinking_mode: false,         // Instruct-2507 has no thinking mode
        prompt_format: PromptFormat::ChatMl,
    },
    OnDeviceModel {
        id: "gemma4-e4b",
        display_name: "Gemma 4 E4B",
        repo_id: "unsloth/gemma-4-E4B-it-GGUF",
        filename: "gemma-4-E4B-it-Q4_K_M.gguf",
        param_count: "4B",
        ram_required_gb: 6.0,
        file_size_gb: 5.0,
        synthesis_token_limit: 6_000,
        context_size: 8_192,
        max_output_tokens: 1_024,
        thinking_mode: false, // thinking stays off through the model's template
        prompt_format: PromptFormat::ModelTemplate,
    },
    OnDeviceModel {
        id: "qwen3.5-9b",
        display_name: "Qwen 3.5 9B",
        repo_id: "unsloth/Qwen3.5-9B-GGUF",
        filename: "Qwen3.5-9B-Q4_K_M.gguf",
        param_count: "9B",
        ram_required_gb: 6.0,
        file_size_gb: 5.5,
        synthesis_token_limit: 16_000, // 128K context, ~12% — DeltaNet arch helps with long ctx
        context_size: 8_192,           // Practical: distillation prompts are 2-4K tokens. 8K gives
        // 6K prompt + 2K output. Same as 4B. Avoids the 4x KV cache
        // overhead of 16K which slows prefill without benefit.
        max_output_tokens: 2_048, // 9B sustains quality up to ~1500 words
        thinking_mode: true,      // defaults to thinking-on
        prompt_format: PromptFormat::ChatMl,
    },
    OnDeviceModel {
        id: "gemma4-12b",
        display_name: "Gemma 4 12B",
        repo_id: "unsloth/gemma-4-12b-it-GGUF",
        filename: "gemma-4-12b-it-Q4_K_M.gguf",
        param_count: "12B",
        ram_required_gb: 9.0,
        file_size_gb: 7.1,
        synthesis_token_limit: 16_000,
        context_size: 8_192,
        max_output_tokens: 2_048,
        thinking_mode: false,
        prompt_format: PromptFormat::ModelTemplate,
    },
    OnDeviceModel {
        id: "qwen3.8-27b",
        display_name: "Qwen 3.8 27B",
        repo_id: "unsloth/Qwen3.8-27B-GGUF",
        filename: "Qwen3.8-27B-UD-Q4_K_M.gguf",
        param_count: "27B",
        ram_required_gb: 19.0,
        file_size_gb: 16.5,
        synthesis_token_limit: 16_000,
        context_size: 8_192,
        max_output_tokens: 2_048,
        thinking_mode: true, // Qwen 3.5-family architecture, thinking-on by default
        prompt_format: PromptFormat::ChatMl,
    },
    OnDeviceModel {
        id: "gemma4-31b",
        display_name: "Gemma 4 31B",
        repo_id: "unsloth/gemma-4-31B-it-GGUF",
        filename: "gemma-4-31B-it-Q4_K_M.gguf",
        param_count: "31B",
        ram_required_gb: 21.0,
        file_size_gb: 18.3,
        synthesis_token_limit: 16_000,
        context_size: 8_192,
        max_output_tokens: 2_048,
        thinking_mode: false,
        prompt_format: PromptFormat::ModelTemplate,
    },
    OnDeviceModel {
        id: "qwen3.6-35b-a3b",
        display_name: "Qwen 3.6 35B-A3B",
        repo_id: "unsloth/Qwen3.6-35B-A3B-GGUF",
        filename: "Qwen3.6-35B-A3B-UD-Q4_K_M.gguf",
        param_count: "35B",
        ram_required_gb: 24.0,
        file_size_gb: 22.1,
        synthesis_token_limit: 16_000,
        context_size: 8_192,
        max_output_tokens: 2_048,
        thinking_mode: true,
        prompt_format: PromptFormat::ChatMl,
    },
];

/// Get model by ID. Returns `None` if the ID is unknown; callers should
/// fall back to `get_default_model()`.
pub fn get_model(id: &str) -> Option<&'static OnDeviceModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// Whether prompts for the model file at `path` should open with an empty
/// think block. Files not in the registry get `false` (no block); see
/// [`OnDeviceModel::thinking_mode`].
pub fn thinking_mode_for_model_file(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    MODELS
        .iter()
        .find(|m| m.filename == name)
        .map(|m| m.thinking_mode)
        .unwrap_or(false)
}

/// How prompts are written for the model file at `path`. Files not in the
/// registry get the model's own template, which is right for any family.
pub fn prompt_format_for_model_file(path: &std::path::Path) -> PromptFormat {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    MODELS
        .iter()
        .find(|m| m.filename == name)
        .map(|m| m.prompt_format)
        .unwrap_or(PromptFormat::ModelTemplate)
}

/// Get the default model (smallest tier, runs on any laptop).
pub fn get_default_model() -> &'static OnDeviceModel {
    &MODELS[0]
}

/// Resolve a possibly-unknown model id to a concrete model, falling back to
/// the default when the id is missing or not in the registry. Used on startup
/// so an outdated `config.on_device_model` value doesn't break initialization.
pub fn resolve_or_default(id: Option<&str>) -> &'static OnDeviceModel {
    match id.and_then(get_model) {
        Some(m) => m,
        None => {
            if let Some(unknown) = id {
                log::warn!(
                    "[on_device_models] unknown model id {:?}, falling back to default {}",
                    unknown,
                    get_default_model().id
                );
            }
            get_default_model()
        }
    }
}

/// Share of total RAM a model may use and still be offered on this computer.
/// Jan's published rule: "Choose models that use less than 80% of your
/// available RAM" (docs.jan.ai/desktop/troubleshooting).
const FIT_SHARE: f64 = 0.8;

/// Whether `model` can run on a computer with `total_ram_gb` of memory.
pub fn fits_ram(model: &OnDeviceModel, total_ram_gb: f64) -> bool {
    model.ram_required_gb <= total_ram_gb * FIT_SHARE
}

/// Recommend a model based on total system RAM (the measured tiers). Used
/// only as the CLI's default answer; the app shows no "recommended" mark.
pub fn recommend_for_ram(total_ram_gb: f64) -> &'static OnDeviceModel {
    if total_ram_gb >= 16.0 {
        &MODELS[2] // qwen3.5-9b
    } else {
        &MODELS[0] // qwen3-4b
    }
}

/// What this computer offers a local model.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalHardware {
    pub total_ram_gb: f64,
    /// CPU and GPU share one memory (Apple Silicon), so no separate VRAM
    /// check applies.
    pub unified_memory: bool,
    /// Largest dedicated GPU memory in GB; `None` when there is no discrete
    /// GPU (or it cannot be read).
    pub dedicated_vram_gb: Option<f64>,
    /// Free disk space where models are downloaded, in GB.
    pub free_disk_gb: Option<f64>,
}

/// How well a model suits this computer, in the three states local-model
/// apps commonly show (Jan: "Fits", "May be slow", "Won't fit").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFit {
    Fits,
    /// Runs, but at least partly on the CPU: Windows/Linux without a discrete
    /// GPU, or a model larger than its memory. LM Studio recommends at least
    /// 4 GB of dedicated VRAM; Ollama runs what does not fit in VRAM on the CPU.
    MaybeSlow,
    /// Needs more than [`FIT_SHARE`] of memory.
    TooLarge,
    /// Not downloaded and the disk has too little free space for it.
    NoDiskSpace,
}

impl ModelFit {
    pub fn can_use(self) -> bool {
        matches!(self, ModelFit::Fits | ModelFit::MaybeSlow)
    }
}

/// Fit of `model` on `hardware`; `cached` models need no download space.
pub fn model_fit(model: &OnDeviceModel, hardware: &LocalHardware, cached: bool) -> ModelFit {
    if !fits_ram(model, hardware.total_ram_gb) {
        return ModelFit::TooLarge;
    }
    if !cached
        && hardware
            .free_disk_gb
            .is_some_and(|free| free < model.file_size_gb)
    {
        return ModelFit::NoDiskSpace;
    }
    if !hardware.unified_memory
        && hardware
            .dedicated_vram_gb
            .is_none_or(|vram| vram < model.ram_required_gb)
    {
        return ModelFit::MaybeSlow;
    }
    ModelFit::Fits
}

/// The few models offered before "Show all": the three largest that fit
/// this computer well, topped up with the smallest usable (slower) ones when
/// fewer fit well. Returned in registry order.
pub fn shortlist(
    hardware: &LocalHardware,
    cached: impl Fn(&OnDeviceModel) -> bool,
) -> Vec<&'static OnDeviceModel> {
    let fits: Vec<(usize, ModelFit)> = MODELS
        .iter()
        .enumerate()
        .map(|(i, m)| (i, model_fit(m, hardware, cached(m))))
        .collect();
    let mut picked: Vec<usize> = fits
        .iter()
        .rev()
        .filter(|(_, fit)| *fit == ModelFit::Fits)
        .map(|(i, _)| *i)
        .take(3)
        .collect();
    for (i, fit) in fits.iter() {
        if picked.len() >= 3 {
            break;
        }
        if fit.can_use() && !picked.contains(i) {
            picked.push(*i);
        }
    }
    picked.sort_unstable();
    picked.into_iter().map(|i| &MODELS[i]).collect()
}

/// Read this computer's hardware: memory, the llama.cpp GPU devices, and
/// free disk space in the model download cache's volume.
pub fn detect_local_hardware() -> Option<LocalHardware> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let bytes = system.total_memory();
    if bytes == 0 {
        return None;
    }
    let gb = |b: u64| b as f64 / (1024.0 * 1024.0 * 1024.0);
    let unified_memory = cfg!(target_os = "macos");
    let dedicated_vram_gb = if unified_memory {
        None
    } else {
        crate::engine::discrete_gpu_memory_bytes().map(gb)
    };
    Some(LocalHardware {
        total_ram_gb: gb(bytes),
        unified_memory,
        dedicated_vram_gb,
        free_disk_gb: free_disk_bytes_for_models().map(gb),
    })
}

/// Free bytes on the disk that holds the Hugging Face cache (the user's home).
fn free_disk_bytes_for_models() -> Option<u64> {
    let home = dirs::home_dir()?;
    fs2::available_space(home).ok()
}

/// Check whether a model is already downloaded to the hf-hub cache.
/// Does not trigger a download. Returns false if the cache can't be located
/// or the file doesn't exist.
pub fn is_cached(model: &OnDeviceModel) -> bool {
    // hf-hub caches at ~/.cache/huggingface/hub/models--{repo_with_slashes_dashified}/
    // snapshots/{hash}/{filename}
    // We scan all snapshot directories for the expected filename.
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let repo_dir = model.repo_id.replace('/', "--");
    let snapshots = home
        .join(".cache")
        .join("huggingface")
        .join("hub")
        .join(format!("models--{}", repo_dir))
        .join("snapshots");
    let Ok(entries) = std::fs::read_dir(&snapshots) else {
        return false;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(model.filename);
        if candidate.exists() {
            return true;
        }
    }
    false
}

/// Resolve an already cached model file without consulting the Hub or changing
/// the cache. The cache API reads the model's existing `refs/main` pointer and
/// snapshot file only; callers decide whether a missing result is fatal.
pub(crate) fn cached_model_path(
    model: &'static OnDeviceModel,
    cache: &hf_hub::Cache,
) -> Option<PathBuf> {
    cache.model(model.repo_id.to_string()).get(model.filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_model_path_uses_existing_hf_snapshot_without_download() {
        let temp = tempfile::tempdir().unwrap();
        let cache = hf_hub::Cache::new(temp.path().to_path_buf());
        let model = get_model("qwen3-4b").unwrap();
        let snapshot = "snapshot-test";
        let snapshot_dir = temp
            .path()
            .join("models--unsloth--Qwen3-4B-Instruct-2507-GGUF")
            .join("snapshots")
            .join(snapshot);
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        let expected = snapshot_dir.join(model.filename);
        std::fs::write(&expected, b"fixture").unwrap();
        cache
            .model(model.repo_id.to_string())
            .create_ref(snapshot)
            .unwrap();

        assert_eq!(cached_model_path(model, &cache), Some(expected));
    }

    #[test]
    fn cached_model_path_returns_none_without_an_existing_snapshot_file() {
        let temp = tempfile::tempdir().unwrap();
        let cache = hf_hub::Cache::new(temp.path().to_path_buf());
        let model = get_model("qwen3-4b").unwrap();

        assert!(cached_model_path(model, &cache).is_none());
        cache
            .model(model.repo_id.to_string())
            .create_ref("snapshot-test")
            .unwrap();
        assert!(cached_model_path(model, &cache).is_none());
    }

    #[test]
    fn thinking_mode_follows_the_model_family() {
        use std::path::Path;
        assert!(!get_model("qwen3-4b").unwrap().thinking_mode);
        assert!(get_model("qwen3.5-9b").unwrap().thinking_mode);
        assert!(!thinking_mode_for_model_file(Path::new(
            "/x/Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
        )));
        assert!(thinking_mode_for_model_file(Path::new(
            "/x/Qwen3.5-9B-Q4_K_M.gguf"
        )));
        assert!(!thinking_mode_for_model_file(Path::new("/x/unknown.gguf")));
    }

    #[test]
    fn registry_is_ordered_by_memory_with_the_default_first() {
        assert_eq!(MODELS[0].id, "qwen3-4b");
        assert!(MODELS
            .windows(2)
            .all(|pair| pair[0].ram_required_gb <= pair[1].ram_required_gb));
        let mut ids: Vec<_> = MODELS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MODELS.len(), "model ids are unique");
    }

    #[test]
    fn non_chatml_families_use_the_model_template() {
        use std::path::Path;
        for model in MODELS {
            let expected = if model.id.starts_with("gemma") {
                PromptFormat::ModelTemplate
            } else {
                PromptFormat::ChatMl
            };
            assert_eq!(model.prompt_format, expected, "{}", model.id);
            assert_eq!(
                prompt_format_for_model_file(Path::new(model.filename)),
                expected
            );
        }
        assert_eq!(
            prompt_format_for_model_file(Path::new("/x/someone-else.gguf")),
            PromptFormat::ModelTemplate
        );
    }

    fn mac(ram: f64) -> LocalHardware {
        LocalHardware {
            total_ram_gb: ram,
            unified_memory: true,
            dedicated_vram_gb: None,
            free_disk_gb: Some(500.0),
        }
    }

    #[test]
    fn fit_follows_memory_disk_and_gpu() {
        let small = get_model("qwen3-4b").unwrap();
        let big = get_model("qwen3.6-35b-a3b").unwrap();
        assert_eq!(model_fit(small, &mac(8.0), false), ModelFit::Fits);
        assert_eq!(model_fit(big, &mac(16.0), false), ModelFit::TooLarge);
        assert_eq!(model_fit(big, &mac(48.0), false), ModelFit::Fits);
        let tight_disk = LocalHardware {
            free_disk_gb: Some(10.0),
            ..mac(48.0)
        };
        assert_eq!(model_fit(big, &tight_disk, false), ModelFit::NoDiskSpace);
        assert_eq!(model_fit(big, &tight_disk, true), ModelFit::Fits);
        let pc = LocalHardware {
            total_ram_gb: 32.0,
            unified_memory: false,
            dedicated_vram_gb: Some(8.0),
            free_disk_gb: Some(500.0),
        };
        assert_eq!(model_fit(small, &pc, false), ModelFit::Fits);
        assert_eq!(
            model_fit(get_model("gemma4-12b").unwrap(), &pc, false),
            ModelFit::MaybeSlow
        );
        let no_gpu = LocalHardware {
            dedicated_vram_gb: None,
            ..pc
        };
        assert_eq!(model_fit(small, &no_gpu, false), ModelFit::MaybeSlow);
    }

    #[test]
    fn shortlist_is_the_three_largest_that_fit_well() {
        let ids = |hw: &LocalHardware| {
            shortlist(hw, |_| false)
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&mac(8.0)), vec!["qwen3-4b", "gemma4-e4b", "qwen3.5-9b"]);
        assert_eq!(
            ids(&mac(16.0)),
            vec!["gemma4-e4b", "qwen3.5-9b", "gemma4-12b"]
        );
        assert_eq!(
            ids(&mac(48.0)),
            vec!["qwen3.8-27b", "gemma4-31b", "qwen3.6-35b-a3b"]
        );
        let pc = LocalHardware {
            total_ram_gb: 32.0,
            unified_memory: false,
            dedicated_vram_gb: Some(4.0),
            free_disk_gb: Some(500.0),
        };
        // Only the smallest fits a 4 GB GPU; the next smallest top it up.
        assert_eq!(ids(&pc), vec!["qwen3-4b", "gemma4-e4b", "qwen3.5-9b"]);
    }

    #[test]
    fn get_model_by_id() {
        assert!(get_model("qwen3-4b").is_some());
        assert!(get_model("qwen3.5-9b").is_some());
        assert!(get_model("nonexistent").is_none());
    }

    #[test]
    fn resolve_or_default_handles_unknown() {
        // Unknown id → default
        assert_eq!(resolve_or_default(Some("qwen3.5-4b")).id, "qwen3-4b");
        assert_eq!(resolve_or_default(Some("nonexistent")).id, "qwen3-4b");
        // None → default
        assert_eq!(resolve_or_default(None).id, "qwen3-4b");
        // Known id → that id
        assert_eq!(resolve_or_default(Some("qwen3.5-9b")).id, "qwen3.5-9b");
    }

    #[test]
    fn recommendation_by_ram() {
        assert_eq!(recommend_for_ram(4.0).id, "qwen3-4b");
        assert_eq!(recommend_for_ram(8.0).id, "qwen3-4b");
        assert_eq!(recommend_for_ram(16.0).id, "qwen3.5-9b");
        assert_eq!(recommend_for_ram(32.0).id, "qwen3.5-9b");
        assert_eq!(recommend_for_ram(64.0).id, "qwen3.5-9b");
    }

    #[test]
    fn default_is_qwen3_4b() {
        assert_eq!(get_default_model().id, "qwen3-4b");
    }
}

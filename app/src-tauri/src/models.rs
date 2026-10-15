//! The brain's model files: the ones Nimbus offers to download, chosen by how much
//! memory the graphics chip (or the processor) has, and any GGUF model the user puts in
//! Nimbus's models folder, which is picked up on its own.
//!
//! The ladder, best first, is PrismML's ternary Bonsai family: the most capable model
//! per gigabyte we know of, with tool calling close to its full-size originals
//! (BFCL v3: 27B 74.9, 8B 73.9, 4B 67.8).

use crate::runtime::Backend;
use nimbus_system::gpu::Vendor;
use serde::Serialize;
use std::fs::File;
use std::io::{BufReader, Read};

type Reader = BufReader<File>;
use std::path::{Path, PathBuf};


/// The server's own buffers and the model's working space, on top of weights and context.
pub const OVERHEAD: u64 = 1_300_000_000;

/// The least context worth running with: Nimbus's instructions and tools take about 4,000
/// tokens, and a conversation needs room after them.
pub const MIN_CONTEXT: u32 = 16_384;

pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub repo: &'static str,
    /// The file for Vulkan, then for every other build, with their sizes.
    vulkan: (&'static str, u64),
    other: (&'static str, u64),
    /// Context memory per token (f16 cache).
    pub kv: u64,
    /// Too slow on a processor alone to answer in conversation.
    gpu_only: bool,
}

pub const LADDER: &[Entry] = &[
    Entry {
        id: "bonsai2-27b",
        name: "Ternary Bonsai 2 27B",
        repo: "prism-ml/Ternary-Bonsai-2-27B-gguf",
        vulkan: ("Ternary-Bonsai-2-27B-PQ2_0.gguf", 7_206_168_928),
        other: ("Ternary-Bonsai-2-27B-PQ2_0.gguf", 7_206_168_928),
        kv: 65_536,
        gpu_only: true,
    },
    // The same model packed tighter, for cards with 10 to 12 GB.
    Entry {
        id: "bonsai2-27b-compact",
        name: "Ternary Bonsai 2 27B (compact)",
        repo: "prism-ml/Ternary-Bonsai-2-27B-gguf",
        vulkan: ("Ternary-Bonsai-2-27B-PTQ1_0.gguf", 5_946_648_928),
        other: ("Ternary-Bonsai-2-27B-PTQ1_0.gguf", 5_946_648_928),
        kv: 65_536,
        gpu_only: true,
    },
    Entry {
        id: "bonsai-8b",
        name: "Ternary Bonsai 8B",
        repo: "prism-ml/Ternary-Bonsai-8B-gguf",
        vulkan: ("Ternary-Bonsai-8B-Q2_0_g64.gguf", 2_310_000_000),
        other: ("Ternary-Bonsai-8B-PQ2_0.gguf", 2_180_000_000),
        kv: 147_456,
        gpu_only: false,
    },
    Entry {
        id: "bonsai-4b",
        name: "Ternary Bonsai 4B",
        repo: "prism-ml/Ternary-Bonsai-4B-gguf",
        vulkan: ("Ternary-Bonsai-4B-Q2_0_g64.gguf", 1_140_000_000),
        other: ("Ternary-Bonsai-4B-PQ2_0.gguf", 1_070_000_000),
        kv: 147_456,
        gpu_only: false,
    },
    Entry {
        id: "bonsai-1.7b",
        name: "Ternary Bonsai 1.7B",
        repo: "prism-ml/Ternary-Bonsai-1.7B-gguf",
        vulkan: ("Ternary-Bonsai-1.7B-Q2_0_g64.gguf", 490_000_000),
        other: ("Ternary-Bonsai-1.7B-PQ2_0.gguf", 460_000_000),
        kv: 114_688,
        gpu_only: false,
    },
];

impl Entry {
    /// The file to use with this build. Vulkan runs the g64 packing that PrismML
    /// supports there; the 27B has only its own two packings.
    pub fn file(&self, b: Backend) -> (&'static str, u64) {
        if b == Backend::Vulkan { self.vulkan } else { self.other }
    }

    pub fn needs(&self, b: Backend, context: u32) -> u64 {
        self.file(b).1 + OVERHEAD + self.kv * context as u64
    }

    fn suits(&self, b: Backend, vendor: Option<Vendor>) -> bool {
        // PTQ1_0 on Vulkan hangs Intel Arc chips after about 1,900 tokens.
        let hangs = self.id == "bonsai2-27b-compact" && b == Backend::Vulkan && vendor == Some(Vendor::Intel);
        !hangs && (b.on_gpu() || !self.gpu_only)
    }
}

/// The best model that fits in `usable` bytes with a workable context. The smallest one
/// if nothing does: llama.cpp then keeps part of it on the processor.
pub fn recommend(b: Backend, vendor: Option<Vendor>, usable: u64) -> &'static Entry {
    LADDER
        .iter()
        .filter(|e| e.suits(b, vendor))
        .find(|e| e.needs(b, MIN_CONTEXT) <= usable)
        .unwrap_or(&LADDER[LADDER.len() - 1])
}

/// The context to run with: what the settings ask for, cut to what the model was
/// trained for and what fits beside it, in whole thousands.
pub fn context_for(size: u64, kv: u64, trained: u32, usable: u64, wanted: u32) -> u32 {
    let room = usable.saturating_sub(size + OVERHEAD) / kv.max(1);
    let n = (wanted as u64).min(trained.max(8_192) as u64).min(room).max(8_192);
    (n / 1024 * 1024) as u32
}

// ---- what a GGUF file says about itself ----

/// The parts of a GGUF file's header that matter for running it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Info {
    pub architecture: String,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub context: Option<u32>,
    /// Context memory per token with an f16 cache, when the header tells enough.
    pub kv: Option<u64>,
}

impl Info {
    /// A language model to chat with, not a vision tower, an embedder or a speech model.
    pub fn is_chat(&self) -> bool {
        let a = self.architecture.as_str();
        let other = a.contains("bert") || a == "clip" || a.starts_with("whisper") || a == "t5encoder" || a.contains("wavtokenizer");
        !other && !matches!(self.kind.as_deref(), Some("mmproj" | "adapter" | "imatrix"))
    }
}

enum Value {
    Int(i64),
    Text(String),
    Ints(Vec<i64>),
    Other,
}

fn read_n<const N: usize>(r: &mut impl Read) -> std::io::Result<[u8; N]> {
    let mut b = [0u8; N];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn read_u64(r: &mut impl Read) -> std::io::Result<u64> {
    Ok(u64::from_le_bytes(read_n::<8>(r)?))
}

fn read_text(r: &mut Reader) -> std::io::Result<Option<String>> {
    let n = read_u64(r)?;
    if n > 1 << 16 {
        r.seek_relative(n as i64)?;
        return Ok(None);
    }
    let mut b = vec![0u8; n as usize];
    r.read_exact(&mut b)?;
    Ok(Some(String::from_utf8_lossy(&b).into_owned()))
}

fn scalar(r: &mut Reader, t: u32) -> std::io::Result<Value> {
    Ok(match t {
        0 => Value::Int(read_n::<1>(r)?[0] as i64),
        1 => Value::Int(read_n::<1>(r)?[0] as i8 as i64),
        2 => Value::Int(u16::from_le_bytes(read_n(r)?) as i64),
        3 => Value::Int(i16::from_le_bytes(read_n(r)?) as i64),
        4 => Value::Int(u32::from_le_bytes(read_n(r)?) as i64),
        5 => Value::Int(i32::from_le_bytes(read_n(r)?) as i64),
        6 => {
            read_n::<4>(r)?;
            Value::Other
        }
        7 => Value::Int(read_n::<1>(r)?[0] as i64),
        8 => read_text(r)?.map(Value::Text).unwrap_or(Value::Other),
        10 => Value::Int(read_u64(r)? as i64),
        11 => Value::Int(read_u64(r)? as i64),
        12 => {
            read_n::<8>(r)?;
            Value::Other
        }
        _ => return Err(std::io::Error::other("unknown GGUF value type")),
    })
}

fn value(r: &mut Reader, t: u32) -> std::io::Result<Value> {
    if t != 9 {
        return scalar(r, t);
    }
    let item = u32::from_le_bytes(read_n(r)?);
    let n = read_u64(r)?;
    let width = match item {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        _ => 0,
    };
    if item == 8 {
        // Lists of words (the tokenizer's 150,000 of them): skipped.
        for _ in 0..n {
            let len = read_u64(r)?;
            r.seek_relative(len as i64)?;
        }
        return Ok(Value::Other);
    }
    if width == 0 {
        return Err(std::io::Error::other("nested GGUF lists"));
    }
    if n <= 1024 && item != 6 && item != 12 {
        return (0..n).map(|_| scalar(r, item).map(|v| if let Value::Int(i) = v { i } else { 0 })).collect::<Result<_, _>>().map(Value::Ints);
    }
    r.seek_relative((width * n) as i64)?;
    Ok(Value::Other)
}

/// Read a GGUF file's header (only the header: a few hundred kilobytes at most).
pub fn info(path: &Path) -> Option<Info> {
    let mut r = BufReader::with_capacity(1 << 16, File::open(path).ok()?);
    if &read_n::<4>(&mut r).ok()? != b"GGUF" {
        return None;
    }
    let _version = u32::from_le_bytes(read_n(&mut r).ok()?);
    let _tensors = read_u64(&mut r).ok()?;
    let count = read_u64(&mut r).ok()?;
    let mut all: Vec<(String, Value)> = Vec::new();
    for _ in 0..count.min(100_000) {
        let key = read_text(&mut r).ok()??;
        let t = u32::from_le_bytes(read_n(&mut r).ok()?);
        let v = value(&mut r, t).ok()?;
        all.push((key, v));
    }
    let get = |k: &str| all.iter().find(|(key, _)| key == k).map(|(_, v)| v);
    let int = |k: &str| match get(k) {
        Some(Value::Int(i)) => Some(*i),
        _ => None,
    };
    let text = |k: &str| match get(k) {
        Some(Value::Text(s)) => Some(s.clone()),
        _ => None,
    };
    let arch = text("general.architecture")?;
    let a = |k: &str| format!("{arch}.{k}");
    let layers = int(&a("block_count")).unwrap_or(0).max(0) as u64;
    let heads = int(&a("attention.head_count")).unwrap_or(0) as u64;
    let width = int(&a("embedding_length")).unwrap_or(0) as u64;
    let head_dim = if heads > 0 { width / heads } else { 0 };
    let key_len = int(&a("attention.key_length")).map(|v| v as u64).unwrap_or(head_dim);
    let val_len = int(&a("attention.value_length")).map(|v| v as u64).unwrap_or(head_dim);
    // Heads with a cache, layer by layer: one number for all layers, or a list where
    // layers without attention (the linear ones of hybrid models) have none.
    let per_layer: u64 = match get(&a("attention.head_count_kv")) {
        Some(Value::Int(n)) => {
            // Hybrid models: only every n-th layer attends over the whole context.
            let every = int(&a("full_attention_interval")).filter(|n| *n > 0).unwrap_or(1) as u64;
            (*n).max(0) as u64 * (layers / every).max(1)
        }
        Some(Value::Ints(v)) => v.iter().map(|n| (*n).max(0) as u64).sum(),
        _ => heads * layers,
    };
    let kv = per_layer * (key_len + val_len) * 2;
    Some(Info {
        name: text("general.name").filter(|n| !n.is_empty() && n != "Hf"),
        kind: text("general.type"),
        context: int(&a("context_length")).map(|c| c.clamp(0, u32::MAX as i64) as u32),
        kv: (kv > 0).then_some(kv),
        architecture: arch,
    })
}

// ---- finding the files ----

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub path: String,
    /// The file's name without ".gguf".
    pub name: String,
    pub size: u64,
    /// The ladder entry it is, if it is one.
    pub known: Option<&'static str>,
    pub context: Option<u32>,
    pub kv: Option<u64>,
}

impl Found {
    pub fn needs(&self, context: u32) -> u64 {
        self.size + OVERHEAD + self.kv.unwrap_or(147_456) * context as u64
    }
}

/// The vision file (mmproj) made for the model at `path`: beside it or one folder down,
/// named after the same model ("Ternary-Bonsai-2-27B-mmproj-Q8_0.gguf" for
/// "Ternary-Bonsai-2-27B-PQ2_0.gguf"). An 8-bit one is preferred to a bigger one.
pub fn vision_for(path: &str) -> Option<String> {
    let p = Path::new(path);
    let stem = p.file_stem()?.to_str()?.to_lowercase();
    // The model's name without its packing: everything before the last "-".
    let family = stem.rsplit_once('-').map_or(stem.as_str(), |(head, _)| head).to_string();
    let dir = p.parent()?;
    let mut places = vec![dir.to_path_buf()];
    if let Ok(rd) = std::fs::read_dir(dir) {
        places.extend(rd.flatten().map(|e| e.path()).filter(|d| d.is_dir()));
    }
    let mut found: Vec<PathBuf> = places
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|f| {
            let n = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
            n.ends_with(".gguf") && n.contains("mmproj") && n.starts_with(&family)
        })
        .collect();
    found.sort_by_key(|f| !f.to_string_lossy().to_lowercase().contains("q8"));
    found.into_iter().next().map(|f| f.to_string_lossy().into_owned())
}

fn split_part(name: &str) -> Option<u32> {
    // "model-00002-of-00004.gguf": only the first part is listed.
    let stem = name.strip_suffix(".gguf")?;
    let (head, _) = stem.rsplit_once("-of-")?;
    head.rsplit_once('-')?.1.parse().ok()
}

/// Every chat model in `dir` and its folders (two deep). `only_bonsai` limits the
/// search to Bonsai files, for folders that are not Nimbus's own (Downloads).
pub fn find_in(dir: &Path, depth: u8, only_bonsai: bool, out: &mut Vec<Found>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                find_in(&p, depth - 1, only_bonsai, out);
            }
            continue;
        }
        let file = p.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let lower = file.to_lowercase();
        if !lower.ends_with(".gguf") || lower.contains("mmproj") || lower.contains("dspark") || split_part(&lower).is_some_and(|n| n != 1) {
            continue;
        }
        if only_bonsai && !lower.contains("bonsai") {
            continue;
        }
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        if size < 50_000_000 {
            continue; // vocabularies and test files
        }
        let Some(info) = info(&p) else { continue };
        if !info.is_chat() {
            continue;
        }
        let known = LADDER.iter().find(|l| l.vulkan.0.eq_ignore_ascii_case(&file) || l.other.0.eq_ignore_ascii_case(&file)).map(|l| l.id);
        if out.iter().any(|f| f.path == p.to_string_lossy()) {
            continue;
        }
        out.push(Found {
            path: p.to_string_lossy().into_owned(),
            name: file.trim_end_matches(".gguf").trim_end_matches(".GGUF").to_string(),
            size,
            known,
            context: info.context,
            kv: info.kv,
        });
    }
}

/// Nimbus's own models folder.
pub fn folder(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    app.path().app_local_data_dir().ok().map(|d| d.join("models"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Every model Nimbus can run: its models folder first, then Bonsai files in Downloads
/// and ~/models, where they were put before the folder existed.
pub fn discover(app: &tauri::AppHandle) -> Vec<Found> {
    let mut out = Vec::new();
    if let Some(dir) = folder(app) {
        let _ = std::fs::create_dir_all(&dir);
        find_in(&dir, 2, false, &mut out);
    }
    if let Some(h) = home() {
        find_in(&h.join("Downloads"), 0, true, &mut out);
        find_in(&h.join("models"), 1, true, &mut out);
    }
    out
}

/// Which found model to run when none is chosen: the best ladder model that fits, else
/// the biggest other model that fits, else the smallest of all.
pub fn pick<'a>(found: &'a [Found], b: Backend, vendor: Option<Vendor>, usable: u64) -> Option<&'a Found> {
    let fits = |f: &&Found| f.needs(MIN_CONTEXT) <= usable;
    LADDER
        .iter()
        .filter(|e| e.suits(b, vendor))
        .find_map(|e| found.iter().filter(fits).find(|f| f.known == Some(e.id)))
        .or_else(|| found.iter().filter(fits).max_by_key(|f| f.size))
        .or_else(|| found.iter().min_by_key(|f| f.size))
}

/// Where a ladder model's file comes from, and its SHA-256 from Hugging Face's list.
pub fn source(e: &Entry, b: Backend) -> Result<(String, Option<String>, u64), String> {
    let (file, size) = e.file(b);
    let list = crate::fetch::json(&format!("https://huggingface.co/api/models/{}/tree/main", e.repo))?;
    let item = list.as_array().into_iter().flatten().find(|i| i["path"] == file);
    let sha = item.and_then(|i| i["lfs"]["oid"].as_str()).map(str::to_string);
    let size = item.and_then(|i| i["size"].as_u64()).unwrap_or(size);
    Ok((format!("https://huggingface.co/{}/resolve/main/{file}", e.repo), sha, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn ladder_by_memory() {
        let pick = |b, v, gb: f64| recommend(b, v, (gb * GB as f64) as u64).id;
        assert_eq!(pick(Backend::Vulkan, Some(Vendor::Amd), 57.0), "bonsai2-27b");
        assert_eq!(pick(Backend::Cuda, Some(Vendor::Nvidia), 21.6), "bonsai2-27b");
        // 12 GB card (a tenth kept free): the full packing with about 35,000 tokens.
        assert_eq!(pick(Backend::Cuda, Some(Vendor::Nvidia), 10.8), "bonsai2-27b");
        assert_eq!(pick(Backend::Cuda, Some(Vendor::Nvidia), 9.0), "bonsai2-27b-compact");
        assert_eq!(pick(Backend::Vulkan, Some(Vendor::Intel), 9.0), "bonsai-8b", "PTQ1_0 hangs Arc");
        assert_eq!(pick(Backend::Cuda, Some(Vendor::Nvidia), 7.2), "bonsai-8b");
        assert_eq!(pick(Backend::Vulkan, Some(Vendor::Amd), 5.0), "bonsai-4b");
        assert_eq!(pick(Backend::Vulkan, Some(Vendor::Amd), 3.6), "bonsai-1.7b");
        assert_eq!(pick(Backend::Vulkan, Some(Vendor::Intel), 2.0), "bonsai-1.7b");
        assert_eq!(pick(Backend::Cpu, None, 30.0), "bonsai-8b", "27B is too slow on a processor");
    }

    #[test]
    fn context_fits() {
        // 57 GB on this computer: the settings' 125,000 tokens fit.
        assert_eq!(context_for(7_206_168_928, 65_536, 262_144, 57 * GB, 125_000), 124_928);
        // 12 GB card: what is left after weights and buffers.
        let c = context_for(7_206_168_928, 65_536, 262_144, 10_800_000_000, 125_000);
        assert!((30_000..40_000).contains(&c), "{c}");
        // Never past what the model was trained for, never under 8,192.
        assert_eq!(context_for(1_070_000_000, 147_456, 32_768, 57 * GB, 125_000), 32_768);
        assert_eq!(context_for(7 * GB, 65_536, 262_144, 7 * GB, 125_000), 8_192);
    }

    #[test]
    fn split_files() {
        assert_eq!(split_part("qwen-00001-of-00003.gguf"), Some(1));
        assert_eq!(split_part("qwen-00002-of-00003.gguf"), Some(2));
        assert_eq!(split_part("qwen3-8b-q4_k_m.gguf"), None);
    }

    /// A GGUF header written by hand: a hybrid model with every fourth layer attending.
    #[test]
    fn reads_a_header() {
        let mut b: Vec<u8> = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        let kvs: Vec<(&str, u32, Vec<u8>)> = vec![
            ("general.architecture", 8, [7u64.to_le_bytes().to_vec(), b"qwen35x".to_vec()].concat()),
            ("general.name", 8, [4u64.to_le_bytes().to_vec(), b"Test".to_vec()].concat()),
            ("qwen35x.block_count", 4, 64u32.to_le_bytes().to_vec()),
            ("qwen35x.context_length", 4, 262_144u32.to_le_bytes().to_vec()),
            ("qwen35x.attention.head_count", 4, 24u32.to_le_bytes().to_vec()),
            ("qwen35x.attention.head_count_kv", 4, 4u32.to_le_bytes().to_vec()),
            ("qwen35x.embedding_length", 4, 5120u32.to_le_bytes().to_vec()),
            ("qwen35x.attention.key_length", 4, 256u32.to_le_bytes().to_vec()),
            ("qwen35x.attention.value_length", 4, 256u32.to_le_bytes().to_vec()),
            ("qwen35x.full_attention_interval", 4, 4u32.to_le_bytes().to_vec()),
            (
                "tokenizer.ggml.tokens",
                9,
                [8u32.to_le_bytes().to_vec(), 2u64.to_le_bytes().to_vec(), 1u64.to_le_bytes().to_vec(), b"a".to_vec(), 1u64.to_le_bytes().to_vec(), b"b".to_vec()].concat(),
            ),
        ];
        b.extend((kvs.len() as u64).to_le_bytes());
        for (k, t, v) in kvs {
            b.extend((k.len() as u64).to_le_bytes());
            b.extend(k.as_bytes());
            b.extend(t.to_le_bytes());
            b.extend(v);
        }
        let p = std::env::temp_dir().join(format!("nimbus-gguf-{}.gguf", std::process::id()));
        std::fs::write(&p, b).unwrap();
        let i = info(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert_eq!(i.architecture, "qwen35x");
        assert_eq!(i.name.as_deref(), Some("Test"));
        assert_eq!(i.context, Some(262_144));
        assert_eq!(i.kv, Some(65_536));
        assert!(i.is_chat());
    }

    /// The real Bonsai file on this computer, when it is there.
    #[test]
    #[ignore = "needs the model file"]
    fn reads_bonsai() {
        let p = home().unwrap().join("Downloads/Ternary-Bonsai-2-27B-PQ2_0.gguf");
        let t = std::time::Instant::now();
        let i = info(&p).unwrap();
        println!("{i:?} in {:?}", t.elapsed());
        assert_eq!(i.kv, Some(65_536));
    }
}

//! The runtime and the models: what they are, where they come from, and
//! their download on first use into the app's data folder (`models/`),
//! resumed after an interruption and checked by SHA-256 before use.
//!
//! Every file is pinned to one revision (a model's commit, a wheel's
//! version), so what is checked is what was measured
//! (docs/lyrics-align.md).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::Error;

/// A downloadable part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum ModelId {
    /// ONNX Runtime with DirectML.
    Runtime,
    /// MDX-Net vocal separation.
    Separator,
    /// The multilingual aligner (MMS, romanised letters).
    AlignerMms,
    /// The Russian aligner.
    AlignerRu,
    /// The Japanese aligner.
    AlignerJa,
    /// The English aligner.
    AlignerEn,
    /// Whisper large-v3-turbo, recognition from scratch.
    Whisper,
    /// Mel-Band RoFormer vocal separation (deep mode).
    SeparatorDeep,
    /// GigaAM-v3 CTC, a second Russian aligner (deep mode).
    AlignerRuDeep,
    /// HuBERT large (LibriSpeech 960 h), a second English aligner (deep
    /// mode).
    AlignerEnDeep,
    /// MMS-300m forced aligner fine-tuned on singing (Tune Studio), a
    /// second aligner for romanised letters (deep mode).
    AlignerSung,
}

/// One file of a part.
#[derive(Clone, Copy, Debug)]
pub struct FileSpec {
    /// Its name in the part's folder.
    pub name: &'static str,
    pub url: &'static str,
    /// Bytes of the download.
    pub size: u64,
    pub sha256: &'static str,
    /// The download is a zip (a wheel): only these members are kept, under
    /// their file names.
    pub extract: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub id: ModelId,
    /// Its folder under `models/`.
    pub dir: &'static str,
    pub files: &'static [FileSpec],
}

impl ModelSpec {
    /// Bytes to download.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

const HF: &str = "https://huggingface.co";

macro_rules! hf {
    ($repo:literal, $rev:literal, $path:literal) => {
        concat!("https://huggingface.co/", $repo, "/resolve/", $rev, "/", $path)
    };
}

pub const RUNTIME: ModelSpec = ModelSpec {
    id: ModelId::Runtime,
    dir: "runtime-1.24.4",
    files: &[FileSpec {
        name: "onnxruntime_directml-1.24.4.whl",
        url: "https://files.pythonhosted.org/packages/88/ea/33814eb0ec96775eda4c1d30b0d86e91d7d2cd0d84c66d3915aef0e06fa3/onnxruntime_directml-1.24.4-cp312-cp312-win_amd64.whl",
        size: 25_111_930,
        sha256: "f2ecb68b7b7b259d2ef3112ae760149f9b5a1e7c0fbb73d539da6250a648a614",
        extract: &[
            "onnxruntime/capi/onnxruntime.dll",
            "onnxruntime/capi/onnxruntime_providers_shared.dll",
            "onnxruntime/capi/DirectML.dll",
        ],
    }],
};

pub const SEPARATOR: ModelSpec = ModelSpec {
    id: ModelId::Separator,
    dir: "kim-vocal-2",
    files: &[FileSpec {
        name: "Kim_Vocal_2.onnx",
        url: hf!("seanghay/uvr_models", "6f4fc0cfb0717c9033ffed12471a53008f145b20", "Kim_Vocal_2.onnx"),
        size: 66_759_214,
        sha256: "ce74ef3b6a6024ce44211a07be9cf8bc6d87728cc852a68ab34eb8e58cde9c8b",
        extract: &[],
    }],
};

pub const ALIGNER_MMS: ModelSpec = ModelSpec {
    id: ModelId::AlignerMms,
    dir: "mms-300m-forced-aligner",
    files: &[
        FileSpec {
            name: "model_int8.onnx",
            url: hf!("onnx-community/mms-300m-1130-forced-aligner-ONNX", "2100fb247d8e43962eef24491597fbeb8b469531", "onnx/model_int8.onnx"),
            size: 317_341_664,
            sha256: "2eb5c3d2f6db2ef476aa7a7e1e5800145973e9064eb5292b1b9b8ada1207712a",
            extract: &[],
        },
        FileSpec {
            name: "vocab.json",
            url: hf!("onnx-community/mms-300m-1130-forced-aligner-ONNX", "2100fb247d8e43962eef24491597fbeb8b469531", "vocab.json"),
            size: 351,
            sha256: "0ff83fc9d044537f7c72d92db15b04917f1e50cada799957963bca61e4305971",
            extract: &[],
        },
    ],
};

pub const ALIGNER_RU: ModelSpec = ModelSpec {
    id: ModelId::AlignerRu,
    dir: "xlsr-53-russian",
    files: &[
        FileSpec {
            name: "model_int8.onnx",
            url: hf!("onnx-community/wav2vec2-large-xlsr-53-russian-ONNX", "df5ba1d9e0f9a34c44eec437b441f4ae43c842eb", "onnx/model_int8.onnx"),
            size: 317_349_889,
            sha256: "cbdb8961f4eea83fce782311e3ab6d6581aeceeb8caefa44518a9e73ed35d18a",
            extract: &[],
        },
        FileSpec {
            name: "vocab.json",
            url: hf!("onnx-community/wav2vec2-large-xlsr-53-russian-ONNX", "df5ba1d9e0f9a34c44eec437b441f4ae43c842eb", "vocab.json"),
            size: 468,
            sha256: "94af9bc439fe8a64657b1898505fe2bd1a78e2ef8ad24b7dbdc3c6f921cf0953",
            extract: &[],
        },
    ],
};

pub const ALIGNER_JA: ModelSpec = ModelSpec {
    id: ModelId::AlignerJa,
    dir: "xlsr-53-japanese",
    files: &[
        FileSpec {
            name: "model_int8.onnx",
            url: hf!("XDcobra/wav2vec2-large-xlsr-53-japanese-ONNX", "271aa2cdc012897c9a1b8c950754313832c42fc6", "onnx/model_int8.onnx"),
            size: 357_380_616,
            sha256: "da171cee54c5d69cdb344a35a736c334c155da088a5f15465c2586444003e504",
            extract: &[],
        },
        FileSpec {
            name: "vocab.json",
            url: hf!("XDcobra/wav2vec2-large-xlsr-53-japanese-ONNX", "271aa2cdc012897c9a1b8c950754313832c42fc6", "vocab.json"),
            size: 33_983,
            sha256: "da4da0b5b483881895923eef12a3079605c225ec3b5ed6cb523cf14d363e0d29",
            extract: &[],
        },
    ],
};

pub const ALIGNER_EN: ModelSpec = ModelSpec {
    id: ModelId::AlignerEn,
    dir: "wav2vec2-960h-english",
    files: &[
        FileSpec {
            name: "model_int8.onnx",
            url: hf!("linandrea/wav2vec2-large-960h-lv60-self-onnx", "20eaa6d7ea9cd7abb5460b4a697010ca3562377d", "onnx/model_quantized.onnx"),
            size: 355_006_955,
            sha256: "7c8f4b674d2130375f82fd7d5dcc407d86efcfe8d782c000e9bda526bfa0d818",
            extract: &[],
        },
        FileSpec {
            name: "vocab.json",
            url: hf!("linandrea/wav2vec2-large-960h-lv60-self-onnx", "20eaa6d7ea9cd7abb5460b4a697010ca3562377d", "vocab.json"),
            size: 358,
            sha256: "4178db26b3c7570f6a47f14ac6a1c7b32950b8c2800fb097287e53776934f1c5",
            extract: &[],
        },
    ],
};

pub const WHISPER: ModelSpec = ModelSpec {
    id: ModelId::Whisper,
    dir: "whisper-large-v3-turbo",
    files: &[
        FileSpec {
            name: "encoder_model_int8.onnx",
            url: hf!("onnx-community/whisper-large-v3-turbo_timestamped", "b3f77bf9a8c4d5ea3415827033d1ffea7955fd9a", "onnx/encoder_model_int8.onnx"),
            size: 644_883_258,
            sha256: "3cb43fe7c779110a2f33635fbe24d0db175720834215cb1e4dcc51d4786af156",
            extract: &[],
        },
        FileSpec {
            name: "decoder_model_merged_int8.onnx",
            url: hf!("onnx-community/whisper-large-v3-turbo_timestamped", "b3f77bf9a8c4d5ea3415827033d1ffea7955fd9a", "onnx/decoder_model_merged_int8.onnx"),
            size: 439_997_434,
            sha256: "b4ab1c92a9bcf8c8fdc49deacb57ac51b707d5248bc7868af42371b896c3ae52",
            extract: &[],
        },
        FileSpec {
            name: "tokenizer.json",
            url: hf!("onnx-community/whisper-large-v3-turbo_timestamped", "b3f77bf9a8c4d5ea3415827033d1ffea7955fd9a", "tokenizer.json"),
            size: 2_480_645,
            sha256: "b3c8202bbf06d8ee4232c5984baa563784ac4737e2e7fdc42fa180200d3cfcdb",
            extract: &[],
        },
    ],
};

pub const SEPARATOR_DEEP: ModelSpec = ModelSpec {
    id: ModelId::SeparatorDeep,
    dir: "mel-roformer-kim",
    files: &[FileSpec {
        name: "sep-all-int8.onnx",
        url: hf!("redbaty/ytm-vocal-separator", "d9d3f27d58c4f9c0286a07ed541b18ceaa91f107", "sep-all-int8.onnx"),
        size: 235_623_148,
        sha256: "e9570dac2ae14382c99357a547a6739777a029ad7f2b5f6e6bc22e9b0e7b2580",
        extract: &[],
    }],
};

pub const ALIGNER_RU_DEEP: ModelSpec = ModelSpec {
    id: ModelId::AlignerRuDeep,
    dir: "gigaam-v3-ctc",
    files: &[
        FileSpec {
            name: "model_int8.onnx",
            url: hf!("istupakov/gigaam-v3-onnx", "322c3b29492673eb7d0b434bfa9dfb8653e34d02", "v3_ctc.int8.onnx"),
            size: 224_721_181,
            sha256: "ceb61454e2e1a2dec5872cbac1de0fe0a4271d1148f6b26b5bda53ff30a12acd",
            extract: &[],
        },
        FileSpec {
            name: "vocab.txt",
            url: hf!("istupakov/gigaam-v3-onnx", "322c3b29492673eb7d0b434bfa9dfb8653e34d02", "v3_vocab.txt"),
            size: 198,
            sha256: "a9143c30844d3c0bee3e9e927e4084774eb1b9eeaafc473b2c4521e4911a7c07",
            extract: &[],
        },
    ],
};

pub const ALIGNER_EN_DEEP: ModelSpec = ModelSpec {
    id: ModelId::AlignerEnDeep,
    dir: "hubert-large-ls960",
    files: &[
        FileSpec {
            name: "model.onnx",
            url: hf!("aureortiz/melodex-hubert-large-ls960-ft-onnx", "a9037a27bcd15e5ccc03fe4865088119042650fe", "model.onnx"),
            size: 1_262_262_148,
            sha256: "1c77fe7e8aa0c5204e0d3f04139be7a2d6f7b4a4236fd57b3a653612bcfa105b",
            extract: &[],
        },
        FileSpec {
            name: "vocab.json",
            url: hf!("aureortiz/melodex-hubert-large-ls960-ft-onnx", "a9037a27bcd15e5ccc03fe4865088119042650fe", "vocab.json"),
            size: 293,
            sha256: "756e5535a825a3f81190ea56b364bbbb427b89fd5c67800b6f2b73191bb73c2d",
            extract: &[],
        },
    ],
};

pub const ALIGNER_SUNG: ModelSpec = ModelSpec {
    id: ModelId::AlignerSung,
    dir: "tunestudio-aligner",
    files: &[
        FileSpec {
            name: "aligner.onnx",
            url: hf!("tuneperfect/tunestudio-aligner", "3564dc1bf90f62ce479b93be173a772d6ca8a279", "aligner.onnx"),
            size: 1_262_284_621,
            sha256: "597a1d775df8f7fe3c94310568751ef9253645b4d8cdf21bb7a3da1ab80225a1",
            extract: &[],
        },
        FileSpec {
            name: "vocab.json",
            url: hf!("tuneperfect/tunestudio-aligner", "3564dc1bf90f62ce479b93be173a772d6ca8a279", "vocab.json"),
            size: 384,
            sha256: "7c08508d30f76c24863785b16bfa16477216848b3f1a3f90d5f3bb8c7e1920d1",
            extract: &[],
        },
    ],
};

pub const ALL: [ModelSpec; 11] =
    [RUNTIME, SEPARATOR, ALIGNER_MMS, ALIGNER_RU, ALIGNER_JA, ALIGNER_EN, WHISPER, SEPARATOR_DEEP, ALIGNER_RU_DEEP, ALIGNER_EN_DEEP, ALIGNER_SUNG];

impl ModelId {
    pub fn spec(self) -> ModelSpec {
        ALL.into_iter().find(|s| s.id == self).expect("every id has a spec")
    }

    /// The model file of a part (its first file).
    pub fn model_file(self) -> &'static str {
        self.spec().files[0].name
    }

    /// A part only deep mode uses (docs/lyrics-align.md, «Deep mode»).
    pub fn deep(self) -> bool {
        matches!(self, Self::SeparatorDeep | Self::AlignerRuDeep | Self::AlignerEnDeep | Self::AlignerSung)
    }
}

/// What is on disk of a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Missing,
    /// Bytes downloaded so far (an interrupted download).
    Partial(u64),
    Ready,
}

/// The `models/` folder.
#[derive(Clone, Debug)]
pub struct Catalog {
    dir: PathBuf,
}

/// Marks a part as complete and checked.
const READY: &str = ".ready";

impl Catalog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A part's folder.
    pub fn path(&self, id: ModelId) -> PathBuf {
        self.dir.join(id.spec().dir)
    }

    /// A file of a part.
    pub fn file(&self, id: ModelId, name: &str) -> PathBuf {
        self.path(id).join(name)
    }

    pub fn state(&self, id: ModelId) -> ModelState {
        let dir = self.path(id);
        if dir.join(READY).exists() {
            return ModelState::Ready;
        }
        let got: u64 = id
            .spec()
            .files
            .iter()
            .map(|f| fs::metadata(dir.join(f.name)).or_else(|_| fs::metadata(part_path(&dir, f.name))).map_or(0, |m| m.len()))
            .sum();
        if got == 0 { ModelState::Missing } else { ModelState::Partial(got) }
    }

    pub fn ready(&self, id: ModelId) -> bool {
        self.state(id) == ModelState::Ready
    }

    /// Bytes a part takes on disk.
    pub fn disk_size(&self, id: ModelId) -> u64 {
        fs::read_dir(self.path(id))
            .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
            .unwrap_or(0)
    }

    /// Deletes a part.
    pub fn remove(&self, id: ModelId) -> Result<(), Error> {
        let dir = self.path(id);
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    /// Downloads what a part lacks. `progress(done, total)` is called as
    /// bytes come; returning false stops (what came stays for a resume).
    pub fn download(&self, id: ModelId, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<(), Error> {
        let spec = id.spec();
        let dir = self.path(id);
        if dir.join(READY).exists() {
            return Ok(());
        }
        fs::create_dir_all(&dir)?;
        let total = spec.size();
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .timeout(None)
            .user_agent("nikgob-music")
            .build()
            .map_err(|e| Error::Download(e.to_string()))?;
        let mut before = 0;
        for f in spec.files {
            let done = |dir: &Path| {
                if f.extract.is_empty() { dir.join(f.name).exists() } else { f.extract.iter().all(|m| dir.join(member_name(m)).exists()) }
            };
            if !done(&dir) {
                let part = part_path(&dir, f.name);
                fetch(&client, f, &part, &mut |got| progress(before + got, total))?;
                check(&part, f)?;
                if f.extract.is_empty() {
                    fs::rename(&part, dir.join(f.name))?;
                } else {
                    unzip(&part, &dir, f.extract)?;
                    fs::remove_file(&part)?;
                }
            }
            before += f.size;
            if !progress(before, total) {
                return Err(Error::Stopped);
            }
        }
        fs::write(dir.join(READY), b"")?;
        Ok(())
    }
}

fn part_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.part"))
}

fn member_name(member: &str) -> &str {
    member.rsplit('/').next().unwrap_or(member)
}

/// Downloads `f` into `part`, going on from what it already has.
fn fetch(client: &reqwest::blocking::Client, f: &FileSpec, part: &Path, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), Error> {
    let mut tries = 0;
    loop {
        let have = fs::metadata(part).map_or(0, |m| m.len());
        if have >= f.size {
            return Ok(());
        }
        let mut req = client.get(f.url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let result = (|| -> Result<(), Error> {
            let mut resp = req.send().map_err(|e| Error::Download(e.to_string()))?;
            let status = resp.status();
            let append = status == reqwest::StatusCode::PARTIAL_CONTENT;
            if !status.is_success() {
                return Err(Error::Download(format!("{} answered {status}", host(f.url))));
            }
            let mut file = fs::OpenOptions::new().create(true).append(append).write(true).truncate(!append).open(part)?;
            let mut got = if append { have } else { 0 };
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = resp.read(&mut buf).map_err(|e| Error::Download(e.to_string()))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])?;
                got += n as u64;
                if !progress(got) {
                    return Err(Error::Stopped);
                }
            }
            file.flush()?;
            Ok(())
        })();
        match result {
            Ok(()) => tries = 0,
            Err(Error::Stopped) => return Err(Error::Stopped),
            Err(e) => {
                // Dropped connections are common on long downloads: go on
                // from where it stopped, a few times.
                tries += 1;
                if tries > 5 {
                    return Err(e);
                }
                log::info!("align: download of {} interrupted ({e}), resuming", f.name);
                std::thread::sleep(Duration::from_secs(2 * tries));
            }
        }
        if fs::metadata(part).map_or(0, |m| m.len()) > f.size {
            fs::remove_file(part)?;
            return Err(Error::Download(format!("{} is larger than expected", f.name)));
        }
    }
}

fn host(url: &str) -> &str {
    url.split('/').nth(2).unwrap_or(url)
}

/// Checks a whole download against its hash (a bad one is deleted).
fn check(part: &Path, f: &FileSpec) -> Result<(), Error> {
    let mut file = fs::File::open(part)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if got != f.sha256 {
        drop(file);
        let _ = fs::remove_file(part);
        return Err(Error::Download(format!("{} is damaged (SHA-256 {got})", f.name)));
    }
    Ok(())
}

fn unzip(zip_path: &Path, dir: &Path, members: &[&str]) -> Result<(), Error> {
    let mut zip = zip::ZipArchive::new(fs::File::open(zip_path)?).map_err(|e| Error::Download(e.to_string()))?;
    for m in members {
        let mut entry = zip.by_name(m).map_err(|e| Error::Download(format!("{m}: {e}")))?;
        let tmp = dir.join(format!("{}.tmp", member_name(m)));
        let mut out = fs::File::create(&tmp)?;
        std::io::copy(&mut entry, &mut out)?;
        drop(out);
        fs::rename(&tmp, dir.join(member_name(m)))?;
    }
    Ok(())
}

#[allow(dead_code)]
fn _hf() -> &'static str {
    HF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_pinned_and_named() {
        for s in ALL {
            assert!(!s.files.is_empty());
            for f in s.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(!f.url.contains("/resolve/main/"), "{} is not pinned", f.url);
                assert!(f.size > 0);
            }
        }
        assert_eq!(member_name("onnxruntime/capi/onnxruntime.dll"), "onnxruntime.dll");
        assert!(WHISPER.size() > 1_000_000_000);
    }

    #[test]
    fn every_aligner_has_its_folder_model_and_letters() {
        let dirs: std::collections::HashSet<&str> = ALL.iter().map(|s| s.dir).collect();
        assert_eq!(dirs.len(), ALL.len(), "two parts share a folder");
        for id in [ModelId::AlignerMms, ModelId::AlignerRu, ModelId::AlignerJa, ModelId::AlignerEn] {
            let names: Vec<&str> = id.spec().files.iter().map(|f| f.name).collect();
            assert_eq!(names, ["model_int8.onnx", "vocab.json"], "{id:?}");
        }
        // The English aligner: wav2vec2 large 960h (lv60, self-training).
        assert!(ALIGNER_EN.files[0].url.contains("wav2vec2-large-960h-lv60-self"));
        assert!(ALIGNER_EN.size() < 360_000_000);
    }

    #[test]
    fn a_missing_part_is_missing_and_a_partial_one_counts_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::new(dir.path());
        assert_eq!(c.state(ModelId::Separator), ModelState::Missing);
        fs::create_dir_all(c.path(ModelId::Separator)).unwrap();
        fs::write(part_path(&c.path(ModelId::Separator), "Kim_Vocal_2.onnx"), [0u8; 10]).unwrap();
        assert_eq!(c.state(ModelId::Separator), ModelState::Partial(10));
        fs::write(c.path(ModelId::Separator).join(READY), b"").unwrap();
        assert!(c.ready(ModelId::Separator));
        c.remove(ModelId::Separator).unwrap();
        assert_eq!(c.state(ModelId::Separator), ModelState::Missing);
    }

    #[test]
    fn a_damaged_download_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.part");
        fs::write(&p, b"hello").unwrap();
        let f = FileSpec { name: "x", url: "https://example.invalid/x", size: 5, sha256: "00", extract: &[] };
        assert!(matches!(check(&p, &f), Err(Error::Download(_))));
        assert!(!p.exists());
        let good = FileSpec { sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824", ..f };
        fs::write(&p, b"hello").unwrap();
        check(&p, &good).unwrap();
    }
}

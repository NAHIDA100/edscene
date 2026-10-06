use crate::config::{Config, Paths};
use crate::event::EventKind;
use crate::presets::def;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Extensions we will pick up from a sound pack. Everything here is *scannable*;
/// whether it also plays is decided by [`Container::sniff`], because a file's
/// extension is a claim, not a fact (the factory pack ships AAC-in-MP4 streams
/// behind a `.mp3` suffix).
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "m4a", "m4b", "mp4", "aac", "wav", "wave", "flac", "ogg", "oga", "opus", "aif", "aiff",
    "wma",
];

/// What the first bytes actually say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp3,
    Mp4,
    Adts,
    Wav,
    Flac,
    Ogg,
    Aiff,
    Unknown,
}

impl Container {
    pub fn label(self) -> &'static str {
        match self {
            Container::Mp3 => "MP3",
            Container::Mp4 => "M4A/MP4",
            Container::Adts => "AAC (ADTS)",
            Container::Wav => "WAV",
            Container::Flac => "FLAC",
            Container::Ogg => "Ogg",
            Container::Aiff => "AIFF",
            Container::Unknown => "未知",
        }
    }

    /// Decodable by the bundled symphonia build.
    pub fn playable(self, head: &[u8]) -> bool {
        match self {
            Container::Mp3 | Container::Mp4 | Container::Adts | Container::Wav => true,
            // An Ogg stream only plays when it carries Vorbis; Opus is not
            // compiled into symphonia, so look at the codec tag in the first
            // packet rather than trusting the extension.
            Container::Ogg => {
                let hay = &head[..head.len().min(64)];
                hay.len() >= 10
                    && hay.windows(6).any(|w| w == *b"vorbis")
                    && !hay.windows(4).any(|w| w == *b"Opus")
            }
            Container::Flac | Container::Aiff => true,
            Container::Unknown => false,
        }
    }

    pub fn sniff(head: &[u8]) -> Container {
        if head.len() >= 3 && head[0..3] == *b"ID3" {
            return Container::Mp3;
        }
        if head.len() >= 4 && head[0..4] == *b"RIFF" && head.len() >= 12 && head[8..12] == *b"WAVE"
        {
            return Container::Wav;
        }
        if head.len() >= 4 && head[0..4] == *b"fLaC" {
            return Container::Flac;
        }
        if head.len() >= 4 && head[0..4] == *b"OggS" {
            return Container::Ogg;
        }
        if head.len() >= 4 && (head[0..4] == *b"FORM") {
            return Container::Aiff;
        }
        if head.len() >= 8 && &head[4..8] == b"ftyp" {
            return Container::Mp4;
        }
        // MPEG frame sync: 11 set bits. 0xFF Ex/Fx are MP3/PAD, 0xFFF1/F9 are
        // typically ADTS AAC.
        if head.len() >= 2 && head[0] == 0xFF && (head[1] & 0xE0) == 0xE0 {
            if head.len() >= 3 && (head[1] & 0x10) == 0x00 {
                return Container::Adts;
            }
            return Container::Mp3;
        }
        Container::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveSource {
    /// A file the app copied into its own override folder.
    Override,
    /// An absolute path chosen per event in the UI.
    Explicit,
    /// The factory preset name matched inside the user's sound pack.
    Pack,
}

impl ResolveSource {
    pub fn label(self) -> &'static str {
        match self {
            ResolveSource::Override => "已替换",
            ResolveSource::Explicit => "手动指定",
            ResolveSource::Pack => "预设",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundStatus {
    Ready,
    /// No sound pack picked yet.
    NotConfigured,
    Missing,
    /// Present, but this build cannot decode it (Opus, WMA, ...).
    Unsupported,
    /// Present, but unreadable (permissions, network share, deleted mid-scan).
    Unreadable,
}

impl SoundStatus {
    pub fn label(self) -> &'static str {
        match self {
            SoundStatus::Ready => "就绪",
            SoundStatus::NotConfigured => "未选择素材目录",
            SoundStatus::Missing => "未找到文件",
            SoundStatus::Unsupported => "格式不支持",
            SoundStatus::Unreadable => "无法读取",
        }
    }

    pub fn is_ok(self) -> bool {
        self == SoundStatus::Ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub kind: EventKind,
    pub path: Option<PathBuf>,
    pub source: Option<ResolveSource>,
    pub container: Container,
    pub status: SoundStatus,
    pub preset_file: &'static str,
    /// (mtime seconds, size bytes) of the resolved file, so an in-place
    /// replacement is noticed without any watcher.
    pub stamp: Option<(u64, u64)>,
}

impl Resolution {
    pub fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| self.preset_file.to_string())
    }
}

fn stamp_of(p: &Path) -> Option<(u64, u64)> {
    let md = std::fs::metadata(p).ok()?;
    let secs = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((secs, md.len()))
}

fn head_of(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut buf = vec![0u8; 64];
    let n = f.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    Some(buf)
}

fn eq_fold(a: &str, b: &str) -> bool {
    a.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).eq(
        b.chars().map(|c| c.to_lowercase().next().unwrap_or(c)),
    )
}

fn stem_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

fn ext_of(name: &str) -> Option<&str> {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => Some(&name[i + 1..]),
        _ => None,
    }
}

fn is_audio_name(name: &str) -> bool {
    ext_of(name)
        .map(|e| AUDIO_EXTS.iter().any(|a| eq_fold(e, a)))
        .unwrap_or(false)
}

/// Find `<names>.<any audio extension>` inside `dir`, preferring the order of
/// `names` and then the order of [`AUDIO_EXTS`] so the result is deterministic.
pub fn find_by_names(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    if names.is_empty() {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut found: Vec<(usize, usize, PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let Some(fname) = e.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if !is_audio_name(&fname) {
            continue;
        }
        let Some(ni) = names.iter().position(|n| eq_fold(stem_of(&fname), n)) else {
            continue;
        };
        let ei = ext_of(&fname)
            .and_then(|x| AUDIO_EXTS.iter().position(|a| eq_fold(x, a)))
            .unwrap_or(99);
        if e.path().is_file() {
            found.push((ni, ei, e.path()));
        }
    }
    found.sort();
    found.into_iter().next().map(|(_, _, p)| p)
}

/// Names a pack entry may legally answer to: the factory file name, its stem,
/// and the ASCII event id.
fn alias_names(kind: EventKind) -> Vec<&'static str> {
    let d = def(kind);
    vec![d.stem(), d.kind.id()]
}

fn classify(path: &Path, source: ResolveSource) -> ResolutionInfo {
    let Some(stamp) = stamp_of(path) else {
        return ResolutionInfo {
            path: Some(path.to_path_buf()),
            source,
            container: Container::Unknown,
            status: SoundStatus::Unreadable,
            stamp: None,
        };
    };
    let head = head_of(path).unwrap_or_default();
    let container = Container::sniff(&head);
    let status = if container.playable(&head) {
        SoundStatus::Ready
    } else {
        SoundStatus::Unsupported
    };
    ResolutionInfo {
        path: Some(path.to_path_buf()),
        source,
        container,
        status,
        stamp: Some(stamp),
    }
}

struct ResolutionInfo {
    path: Option<PathBuf>,
    source: ResolveSource,
    container: Container,
    status: SoundStatus,
    stamp: Option<(u64, u64)>,
}

/// The three layers, highest priority first:
/// 1. the app's override folder (what `替换` writes),
/// 2. an absolute path pinned per event in the config,
/// 3. the user's sound pack, matched by factory name.
///
/// Because layer 3 matches on the *stem*, swapping `锁屏.mp3` for `锁屏.wav` in
/// place keeps the preset working, and because the container is sniffed rather
/// than trusted, a mislabelled file still plays.
pub fn resolve(cfg: &Config, paths: &Paths, kind: EventKind) -> Resolution {
    let d = def(kind);
    let base = |info: Option<ResolutionInfo>| match info {
        Some(i) => Resolution {
            kind,
            path: i.path,
            source: Some(i.source),
            container: i.container,
            status: i.status,
            preset_file: d.preset_file,
            stamp: i.stamp,
        },
        None => Resolution {
            kind,
            path: None,
            source: None,
            container: Container::Unknown,
            status: if cfg.sounds.dir.trim().is_empty() {
                SoundStatus::NotConfigured
            } else {
                SoundStatus::Missing
            },
            preset_file: d.preset_file,
            stamp: None,
        },
    };

    let explicit = cfg.event(kind).file.clone();
    if !explicit.trim().is_empty() {
        let p = PathBuf::from(&explicit);
        if p.is_file() {
            return base(Some(classify(&p, ResolveSource::Explicit)));
        }
        return base(Some(ResolutionInfo {
            path: Some(p),
            source: ResolveSource::Explicit,
            container: Container::Unknown,
            status: SoundStatus::Missing,
            stamp: None,
        }));
    }

    let names = alias_names(kind);
    if let Some(p) = find_by_names(&paths.override_dir, &names) {
        return base(Some(classify(&p, ResolveSource::Override)));
    }
    if !cfg.sounds.dir.trim().is_empty() {
        let dir = PathBuf::from(&cfg.sounds.dir);
        if let Some(p) = find_by_names(&dir, &names) {
            return base(Some(classify(&p, ResolveSource::Pack)));
        }
    }
    base(None)
}

pub fn resolve_all(cfg: &Config, paths: &Paths) -> Vec<Resolution> {
    crate::presets::EVENTS
        .iter()
        .map(|d| resolve(cfg, paths, d.kind))
        .collect()
}

/// How many enabled events currently have something to play.
pub fn readiness(cfg: &Config, paths: &Paths) -> (usize, usize) {
    let enabled = cfg.enabled_events();
    let ok = enabled
        .iter()
        .filter(|k| resolve(cfg, paths, **k).status.is_ok())
        .count();
    (ok, enabled.len())
}

/// Reject paths that would make the service read from somewhere surprising.
pub fn validate_sound_dir(raw: &str, allow_network: bool) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("目录为空".into());
    }
    let p = PathBuf::from(trimmed);
    if !p.is_absolute() {
        return Err("必须使用绝对路径".into());
    }
    if trimmed.contains("..") {
        return Err("路径不能包含 ..".into());
    }
    if !allow_network && (trimmed.starts_with("\\\\") || trimmed.starts_with("//")) {
        return Err("默认不允许网络共享目录，可在高级设置里放开".into());
    }
    if !p.is_dir() {
        return Err("目录不存在".into());
    }
    Ok(p)
}

/// Install a user-supplied clip as the override for one event. Returns the
/// stored path. Any previous override for that event is retired, not deleted.
pub fn install_override(paths: &Paths, kind: EventKind, src: &Path, copy: bool) -> Result<PathBuf, std::io::Error> {
    std::fs::create_dir_all(&paths.override_dir)?;
    retire_overrides(paths, kind)?;
    let ext = ext_of(&src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let dst = paths.override_dir.join(format!("{}.{}", kind.id(), ext));
    if copy {
        std::fs::copy(src, &dst)?;
    } else if std::fs::hard_link(src, &dst).is_err() {
        // Cross-device or a filesystem without hard links: fall back to a copy
        // rather than failing the user's replacement.
        std::fs::copy(src, &dst)?;
    }
    Ok(dst)
}

/// Move any existing override for `kind` into `.disabled` so a factory reset is
/// reversible and the user never loses a recording.
pub fn retire_overrides(paths: &Paths, kind: EventKind) -> Result<usize, std::io::Error> {
    let rd = match std::fs::read_dir(&paths.override_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut n = 0;
    for e in rd.flatten() {
        let Some(name) = e.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if !is_audio_name(&name) {
            continue;
        }
        if stem_of(&name) != kind.id() {
            continue;
        }
        let dest = paths.disabled_dir();
        std::fs::create_dir_all(&dest)?;
        let target = dest.join(&name);
        let _ = std::fs::remove_file(&target);
        if std::fs::rename(e.path(), &target).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

pub fn retire_all_overrides(paths: &Paths) -> Result<usize, std::io::Error> {
    let mut n = 0;
    for k in EventKind::ALL {
        n += retire_overrides(paths, k)?;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "edscene-res-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn paths(root: &Path) -> Paths {
        Paths {
            config: root.join("config.toml"),
            override_dir: root.join("sounds"),
            state_dir: root.join("state"),
        }
    }

    #[test]
    fn sniffs_the_real_factory_pack_and_the_lies_it_tells() {
        // AAC-in-MP4 wearing a .mp3 suffix, exactly like the shipped files.
        let mut f = vec![0u8; 32];
        f[4..8].copy_from_slice(b"ftyp");
        f[8..12].copy_from_slice(b"isom");
        assert_eq!(Container::sniff(&f), Container::Mp4);
        assert!(Container::Mp4.playable(&f));

        let mut id3 = vec![0u8; 32];
        id3[..3].copy_from_slice(b"ID3");
        assert_eq!(Container::sniff(&id3), Container::Mp3);
        assert!(Container::Mp3.playable(&id3));

        let mut riff = vec![0u8; 32];
        riff[..4].copy_from_slice(b"RIFF");
        riff[8..12].copy_from_slice(b"WAVE");
        assert_eq!(Container::sniff(&riff), Container::Wav);

        // Real Opus ID header: "OpusHead" sits at byte 28 of the first packet.
        let mut opus = vec![0u8; 48];
        opus[..4].copy_from_slice(b"OggS");
        opus[28..36].copy_from_slice(b"OpusHead");
        assert_eq!(Container::sniff(&opus), Container::Ogg);
        assert!(!Container::Ogg.playable(&opus), "Opus is not compiled in");

        let mut vorbis = vec![0u8; 48];
        vorbis[..4].copy_from_slice(b"OggS");
        vorbis[28..34].copy_from_slice(b"vorbis");
        assert!(Container::Ogg.playable(&vorbis));

        assert_eq!(Container::sniff(b"\x00\x01\x02\x03"), Container::Unknown);
        assert!(!Container::Unknown.playable(b"\x00\x01\x02\x03"));
    }

    #[test]
    fn pack_lookup_ignores_the_extension() {
        let root = tmp("ext");
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("锁屏.wav"), b"RIFFxxxxWAVE").unwrap();
        let mut cfg = Config::factory();
        cfg.sounds.dir = pack.display().to_string();
        let p = paths(&root);
        let r = resolve(&cfg, &p, EventKind::Lock);
        assert_eq!(r.status, SoundStatus::Ready);
        assert_eq!(r.source, Some(ResolveSource::Pack));
        assert_eq!(r.container, Container::Wav);
        assert_eq!(r.file_name(), "锁屏.wav");
    }

    #[test]
    fn pack_lookup_also_accepts_the_ascii_event_id() {
        let root = tmp("id");
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("unlock.mp3"), b"ID3\x03\x00\x00\x00").unwrap();
        let mut cfg = Config::factory();
        cfg.sounds.dir = pack.display().to_string();
        let r = resolve(&cfg, &paths(&root), EventKind::Unlock);
        assert_eq!(r.status, SoundStatus::Ready);
        assert_eq!(r.file_name(), "unlock.mp3");
    }

    #[test]
    fn override_beats_explicit_beats_pack() {
        let root = tmp("prio");
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("锁屏.mp3"), b"ID3\x03\x00\x00\x00").unwrap();
        let explicit = root.join("picked.m4a");
        std::fs::write(&explicit, b"\x00\x00\x00$fTYP".to_vec()).unwrap();
        let mut cfg = Config::factory();
        cfg.sounds.dir = pack.display().to_string();
        let p = paths(&root);

        assert_eq!(
            resolve(&cfg, &p, EventKind::Lock).source,
            Some(ResolveSource::Pack)
        );

        cfg.event_mut(EventKind::Lock).file = explicit.display().to_string();
        assert_eq!(
            resolve(&cfg, &p, EventKind::Lock).source,
            Some(ResolveSource::Explicit)
        );
        cfg.event_mut(EventKind::Lock).file = String::new();

        std::fs::create_dir_all(&p.override_dir).unwrap();
        std::fs::write(p.override_dir.join("lock.flac"), b"fLaC\x00\x00\x00\x22").unwrap();
        let r = resolve(&cfg, &p, EventKind::Lock);
        assert_eq!(r.source, Some(ResolveSource::Override));
        assert_eq!(r.container, Container::Flac);
    }

    #[test]
    fn replacing_a_file_in_place_changes_the_stamp() {
        let root = tmp("stamp");
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        let f = pack.join("锁屏.mp3");
        std::fs::write(&f, b"ID3\x03\x00\x00\x00").unwrap();
        let mut cfg = Config::factory();
        cfg.sounds.dir = pack.display().to_string();
        let p = paths(&root);
        let before = resolve(&cfg, &p, EventKind::Lock).stamp;
        std::fs::write(&f, b"ID3\x03\x00\x00\x00longer content").unwrap();
        let after = resolve(&cfg, &p, EventKind::Lock).stamp;
        assert!(before.is_some() && after.is_some());
        assert_ne!(before, after, "in-place swaps must be noticed");
    }

    #[test]
    fn missing_and_unconfigured_are_told_apart() {
        let root = tmp("miss");
        let mut cfg = Config::factory();
        let p = paths(&root);
        assert_eq!(resolve(&cfg, &p, EventKind::Lock).status, SoundStatus::NotConfigured);
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        cfg.sounds.dir = pack.display().to_string();
        assert_eq!(resolve(&cfg, &p, EventKind::Lock).status, SoundStatus::Missing);
    }

    #[test]
    fn undecodable_files_are_flagged_not_silently_dropped() {
        let root = tmp("unsup");
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        let mut opus = vec![0u8; 40];
        opus[..4].copy_from_slice(b"OggS");
        opus[28..37].copy_from_slice(b"OpusHead\0");
        std::fs::write(pack.join("锁屏.opus"), &opus).unwrap();
        let mut cfg = Config::factory();
        cfg.sounds.dir = pack.display().to_string();
        let r = resolve(&cfg, &paths(&root), EventKind::Lock);
        assert_eq!(r.status, SoundStatus::Unsupported);
        assert_eq!(r.container, Container::Ogg);
    }

    #[test]
    fn install_and_retire_overrides_keep_the_users_files() {
        let root = tmp("ovr");
        let p = paths(&root);
        let src = root.join("mine.wav");
        std::fs::write(&src, b"RIFFxxxxWAVE").unwrap();
        let dst = install_override(&p, EventKind::Lock, &src, true).unwrap();
        assert!(dst.exists());
        assert_eq!(dst.file_name().unwrap().to_string_lossy(), "lock.wav");

        let src2 = root.join("mine2.mp3");
        std::fs::write(&src2, b"ID3\x03\x00\x00\x00").unwrap();
        install_override(&p, EventKind::Lock, &src2, true).unwrap();
        // The first override was retired, not deleted.
        assert!(p.disabled_dir().join("lock.wav").exists());
        assert_eq!(retire_all_overrides(&p).unwrap(), 1);
        assert!(p.disabled_dir().join("lock.mp3").exists());
        assert!(src.exists() && src2.exists(), "user files are never touched");
    }

    #[test]
    fn sound_dir_validation_rejects_the_surprises() {
        assert!(validate_sound_dir("", false).is_err());
        assert!(validate_sound_dir("eds", false).is_err());
        assert!(validate_sound_dir("D:\\works\\..\\eds", false).is_err());
        assert!(validate_sound_dir("\\\\nas\\pack", false).is_err());
        assert!(validate_sound_dir("\\\\nas\\pack", true).is_err(), "still must exist");
        assert!(validate_sound_dir("C:\\Windows\\Fonts", false).is_ok());
    }

    #[test]
    fn readiness_only_counts_enabled_events() {
        let root = tmp("ready");
        let pack = root.join("pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("解锁返回桌面.mp3"), b"ID3\x03\x00\x00\x00").unwrap();
        let mut cfg = Config::factory();
        cfg.sounds.dir = pack.display().to_string();
        let (ok, total) = readiness(&cfg, &paths(&root));
        assert_eq!(total, 5, "five events ship enabled");
        assert_eq!(ok, 1);
    }
}

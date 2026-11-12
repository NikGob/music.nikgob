//! «Загрузки»: how much space the music takes and what is left of the
//! disks it is on (the page's header line).

/// The music's files and their disks, as last counted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StorageView {
    /// Counted at least once (nothing is shown before).
    pub known: bool,
    /// The library's audio files, bytes.
    pub library: u64,
    /// The lossless originals kept for the backup (the masters folder),
    /// bytes.
    pub masters: u64,
    /// The disks the library and the originals are on, the library's
    /// first; empty when their free space could not be read.
    pub disks: Vec<DiskSpace>,
}

/// One disk that holds music.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskSpace {
    /// How the line names it when there are several: the drive (`C:`) on
    /// Windows, the music folder's name elsewhere.
    pub name: String,
    pub free: u64,
    pub total: u64,
}

impl DiskSpace {
    /// How full the disk is, 0..1.
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.total.saturating_sub(self.free) as f64 / self.total as f64).clamp(0.0, 1.0) as f32
    }
}

impl StorageView {
    /// All the music: the library and the originals.
    pub fn music(&self) -> u64 {
        self.library.saturating_add(self.masters)
    }

    /// «Музыка 41,2 ГБ · свободно 62 ГБ», with each disk's free space
    /// when the music is on several («свободно 62 ГБ (C:) · 410 ГБ (D:)»).
    pub fn line(&self) -> Option<String> {
        if !self.known {
            return None;
        }
        let mut line = format!("Музыка {}", size_ru(self.music()));
        match self.disks.as_slice() {
            [] => {}
            [one] => line.push_str(&format!(" · свободно {}", free_ru(one.free))),
            [first, rest @ ..] => {
                line.push_str(&format!(" · свободно {} ({})", free_ru(first.free), first.name));
                for d in rest {
                    line.push_str(&format!(" · {} ({})", free_ru(d.free), d.name));
                }
            }
        }
        Some(line)
    }

    /// The hover text: the parts of the music and each disk in full.
    pub fn details(&self) -> String {
        let mut lines = vec![format!("Библиотека: {}", size_ru(self.library))];
        if self.masters > 0 {
            lines.push(format!("Оригиналы без потерь: {}", size_ru(self.masters)));
        }
        for d in &self.disks {
            lines.push(format!("{}: свободно {} из {}", d.name, size_ru(d.free), size_ru(d.total)));
        }
        lines.join("\n")
    }
}

const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];

/// `bytes` in binary units (as the system's file managers count them) and
/// the unit: 41,2 ГБ.
fn scaled(bytes: u64) -> (f64, usize) {
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    (v, i)
}

/// With a decimal comma, `decimals` places, no «,0».
fn number_ru(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    match s.split_once('.') {
        Some((whole, frac)) if frac.bytes().all(|b| b == b'0') => whole.to_owned(),
        Some((whole, frac)) => format!("{whole},{frac}"),
        None => s.clone(),
    }
}

/// A size as Russian writes it: «41,2 ГБ», «820 МБ», «1,4 ТБ» (a decimal
/// below 100).
pub fn size_ru(bytes: u64) -> String {
    let (v, i) = scaled(bytes);
    let decimals = if i == 0 || v >= 100.0 { 0 } else { 1 };
    format!("{} {}", number_ru(v, decimals), UNITS[i])
}

/// Free space, rounder: «62 ГБ», «7,5 ГБ» (a decimal below 10).
pub fn free_ru(bytes: u64) -> String {
    let (v, i) = scaled(bytes);
    let decimals = if i == 0 || v >= 10.0 { 0 } else { 1 };
    format!("{} {}", number_ru(v, decimals), UNITS[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    #[test]
    fn sizes_read_the_russian_way() {
        assert_eq!(size_ru(0), "0 Б");
        assert_eq!(size_ru(GB * 412 / 10), "41,2 ГБ");
        assert_eq!(size_ru(GB * 3), "3 ГБ");
        assert_eq!(size_ru(820 << 20), "820 МБ");
        assert_eq!(size_ru(GB * 1434), "1,4 ТБ");
        assert_eq!(free_ru(GB * 624 / 10), "62 ГБ");
        assert_eq!(free_ru(GB * 75 / 10), "7,5 ГБ");
    }

    #[test]
    fn one_disk_or_several() {
        let mut s = StorageView { library: GB * 30, masters: GB * 112 / 10, ..Default::default() };
        assert_eq!(s.line(), None);
        s.known = true;
        assert_eq!(s.line().as_deref(), Some("Музыка 41,2 ГБ"));
        s.disks = vec![DiskSpace { name: "C:".into(), free: GB * 62, total: GB * 237 }];
        assert_eq!(s.line().as_deref(), Some("Музыка 41,2 ГБ · свободно 62 ГБ"));
        s.disks.push(DiskSpace { name: "D:".into(), free: GB * 410, total: GB * 931 });
        assert_eq!(s.line().as_deref(), Some("Музыка 41,2 ГБ · свободно 62 ГБ (C:) · 410 ГБ (D:)"));
        assert!(s.details().contains("Оригиналы без потерь: 11,2 ГБ"));
        assert!((s.disks[0].used_fraction() - 175.0 / 237.0).abs() < 1e-4);
        assert_eq!(DiskSpace::default().used_fraction(), 0.0);
    }
}

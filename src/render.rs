//! PDF に触らない計算（画素数・ページ番号・キャッシュの鍵）。`cargo test` で確かめる。

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::SystemTime;

/// 本体の最大テクスチャサイズ（`obj.getinfo("image_max")` と同じ）
pub const MAX_SIDE: u32 = 16384;
/// 1 ページの画素数の上限（RGBA で 256MB）。大判の図面を高い解像度で描いたときにメモリを食い尽くさない
pub const MAX_PIXELS: u64 = 8192 * 8192;

/// `PdfPage.Size`（DIP = 1/96 インチ。回転を含む）と解像度（DPI）から、描く画素数を決める。
/// 長辺 16384・画素数 8192² を超えるときは縦横比を保って縮める
pub fn target_size(width_dip: f32, height_dip: f32, dpi: f64) -> (u32, u32) {
    let scale = dpi.max(1.0) / 96.0;
    let mut w = (width_dip.max(0.0) as f64 * scale).max(1.0);
    let mut h = (height_dip.max(0.0) as f64 * scale).max(1.0);
    let mut shrunk = false;
    let side = w.max(h);
    if side > MAX_SIDE as f64 {
        let k = MAX_SIDE as f64 / side;
        w *= k;
        h *= k;
        shrunk = true;
    }
    if w * h > MAX_PIXELS as f64 {
        let k = (MAX_PIXELS as f64 / (w * h)).sqrt();
        w *= k;
        h *= k;
        shrunk = true;
    }
    // 縮めたときは切り捨てて上限を超えないようにする。それ以外は四捨五入（PDF のビューアと同じ画素数）
    let snap = |v: f64| if shrunk { v.floor() } else { v.round() };
    let w = (snap(w) as u32).clamp(1, MAX_SIDE);
    let h = (snap(h) as u32).clamp(1, MAX_SIDE);
    (w, h)
}

/// 「ページ」の値（中間点で動かすと小数になる）を、1 始まりのページ番号にする。範囲の外は端に寄せる
pub fn page_number(value: f64, page_count: u32) -> u32 {
    let n = page_count.max(1);
    if !value.is_finite() {
        return 1;
    }
    (value.round().max(1.0) as u64).min(n as u64) as u32
}

/// ファイルが書き換えられたら描き直すための印
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        Some(Self { modified: meta.modified().ok(), len: meta.len() })
    }
}

/// 描き方の指定。同じなら同じ絵になる
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub path: std::path::PathBuf,
    pub stamp: FileStamp,
    /// 「ページ」の値を丸めたもの（ページ数を超えた値もそのまま。描くときに最後のページへ寄せる）
    pub page: u32,
    pub dpi: u32,
    /// None なら透明（紙の白も描かない）
    pub background: Option<(u8, u8, u8)>,
}

/// 描画結果を変える修正を入れたら上げる（古いキャッシュを使わない）
const CACHE_NS: &str = "pdf_page_h:v1:";

impl RenderKey {
    pub fn cache_name(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        format!("{CACHE_NS}{:016x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slide_at_192dpi_is_fhd() {
        // 16:9 のスライド（720 x 405 pt）は Size が 960 x 540 DIP。192 DPI で 1920 x 1080
        assert_eq!(target_size(960.0, 540.0, 192.0), (1920, 1080));
        assert_eq!(target_size(960.0, 540.0, 96.0), (960, 540));
    }

    #[test]
    fn a4_portrait_and_landscape() {
        // A4（595 x 842 pt = 793.3 x 1122.7 DIP）。プローブで描いたときの画素数と同じ
        assert_eq!(target_size(793.33, 1122.67, 192.0), (1587, 2245));
        assert_eq!(target_size(1122.67, 793.33, 192.0), (2245, 1587));
    }

    #[test]
    fn clamps_to_max_side_and_pixels() {
        // A0 横（1189 x 841 mm）を 600 DPI: 28087 x 19866 → 長辺 16384 → 画素数 8192² に収める
        let (w, h) = target_size(4494.0, 3179.0, 600.0);
        assert!(w <= MAX_SIDE && h <= MAX_SIDE);
        assert!((w as u64) * (h as u64) <= MAX_PIXELS);
        let ratio = w as f64 / h as f64;
        assert!((ratio - 4494.0 / 3179.0).abs() < 0.01, "縦横比が保たれる: {ratio}");
        // 細長いページでも 0 にならない
        let (w, h) = target_size(100000.0, 1.0, 72.0);
        assert_eq!(w, MAX_SIDE);
        assert_eq!(h, 1);
    }

    #[test]
    fn page_number_rounds_and_clamps() {
        assert_eq!(page_number(1.0, 10), 1);
        assert_eq!(page_number(2.4, 10), 2);
        assert_eq!(page_number(2.5, 10), 3);
        assert_eq!(page_number(0.0, 10), 1);
        assert_eq!(page_number(-5.0, 10), 1);
        assert_eq!(page_number(11.0, 10), 10);
        assert_eq!(page_number(9999.0, 3), 3);
        assert_eq!(page_number(f64::NAN, 3), 1);
        assert_eq!(page_number(5.0, 0), 1);
    }

    #[test]
    fn cache_name_changes_with_every_field() {
        let base = RenderKey {
            path: "C:/a.pdf".into(),
            stamp: FileStamp { modified: None, len: 10 },
            page: 1,
            dpi: 192,
            background: Some((255, 255, 255)),
        };
        let name = base.cache_name();
        assert!(name.starts_with(CACHE_NS));
        let variants = [
            RenderKey { path: "C:/b.pdf".into(), ..base.clone() },
            RenderKey { stamp: FileStamp { modified: None, len: 11 }, ..base.clone() },
            RenderKey { page: 2, ..base.clone() },
            RenderKey { dpi: 193, ..base.clone() },
            RenderKey { background: None, ..base.clone() },
            RenderKey { background: Some((0, 0, 0)), ..base.clone() },
        ];
        for v in variants {
            assert_ne!(v.cache_name(), name, "{v:?}");
        }
        assert_eq!(base.clone().cache_name(), name);
    }

    #[test]
    fn stamp_follows_file_edits() {
        let path = std::env::temp_dir().join(format!("pdf_page_h_stamp_{}.pdf", std::process::id()));
        std::fs::write(&path, b"%PDF-1.4").unwrap();
        let before = FileStamp::of(&path).unwrap();
        std::fs::write(&path, b"%PDF-1.4\n%%EOF").unwrap();
        let after = FileStamp::of(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_ne!(before, after);
        assert!(FileStamp::of(&path).is_none());
    }
}

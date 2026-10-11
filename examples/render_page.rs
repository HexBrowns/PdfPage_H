//! 机上の検証用: プラグインと同じ描き方（worker::render）で 1 ページを描き、生の RGBA を書く。
//! 使い方: render_page <pdf> <ページ> <解像度> <背景色 rrggbb か none> <出力>
//! 出力は 先頭 8 バイトに幅と高さ（u32 LE）、続けてストレートのアルファの RGBA。
use std::io::Write;
use std::time::Duration;

use pdf_page_h::render::FileStamp;
use pdf_page_h::worker;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&a[1]);
    let page: u32 = a[2].parse().unwrap();
    let dpi: u32 = a[3].parse().unwrap();
    let bg = if a[4] == "none" {
        None
    } else {
        let v = u32::from_str_radix(&a[4], 16).unwrap();
        Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
    };
    let result = FileStamp::of(&path)
        .ok_or_else(|| "ファイルが無い".to_string())
        .and_then(|stamp| worker::render(&path, stamp, page, dpi, bg, Duration::from_secs(10)));
    worker::shutdown();
    match result {
        Ok(img) => {
            let mut f = std::fs::File::create(&a[5]).unwrap();
            f.write_all(&img.width.to_le_bytes()).unwrap();
            f.write_all(&img.height.to_le_bytes()).unwrap();
            f.write_all(&img.rgba).unwrap();
            println!("ok {}x{}", img.width, img.height);
        }
        Err(e) => {
            println!("err {e}");
            std::process::exit(2);
        }
    }
}

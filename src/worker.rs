//! PDF を描く裏のスレッド（Windows.Data.Pdf）。
//!
//! WinRT は本体のレンダースレッドで初期化しない（本体の COM の初期化と衝突させないため。推測による用心）。
//! このスレッドだけで MTA として初期化し、開いた PDF もこのスレッドの中だけで持つ。
//! プラグインが外れる前に `shutdown` で止めて終わりを待つ（`au2-rs-plugin`「裏のスレッドは…止めて終わりを待つ」）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Graphics::Imaging::{
    BitmapAlphaMode, BitmapDecoder, BitmapPixelFormat, BitmapTransform, ColorManagementMode,
    ExifOrientationMode,
};
use windows::Storage::StorageFile;
use windows::Storage::Streams::InMemoryRandomAccessStream;
use windows::UI::Color;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::HSTRING;

use crate::render::{FileStamp, page_number, target_size};

/// 開いたままにしておく PDF の数
const OPEN_DOCS: usize = 4;

/// 描けたページ。画素はストレートのアルファの RGBA（本体の `PIXEL_RGBA` と同じ並び）
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

struct Request {
    path: PathBuf,
    stamp: FileStamp,
    page: u32,
    dpi: u32,
    background: Option<(u8, u8, u8)>,
    reply: mpsc::Sender<Result<Rendered, String>>,
}

struct Worker {
    tx: mpsc::Sender<Request>,
    handle: JoinHandle<()>,
}

/// None は「まだ起こしていない」か「止めた後」。止めた後は起こし直さない
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
static STOPPED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn sender() -> Result<mpsc::Sender<Request>, String> {
    if STOPPED.load(std::sync::atomic::Ordering::Acquire) {
        return Err("PdfPage_H は終了処理中".into());
    }
    let mut guard = WORKER.lock().map_err(|_| "PdfPage_H の裏のスレッドの状態が壊れた".to_string())?;
    if guard.is_none() {
        let (tx, rx) = mpsc::channel::<Request>();
        let handle = std::thread::Builder::new()
            .name("PdfPage_H worker".into())
            .spawn(move || run(rx))
            .map_err(|e| format!("PdfPage_H の裏のスレッドを起こせない: {e}"))?;
        *guard = Some(Worker { tx, handle });
    }
    Ok(guard.as_ref().unwrap().tx.clone())
}

/// ページを描く。裏のスレッドが描き終えるまで `timeout` だけ待つ
pub fn render(
    path: &Path,
    stamp: FileStamp,
    page: u32,
    dpi: u32,
    background: Option<(u8, u8, u8)>,
    timeout: Duration,
) -> Result<Rendered, String> {
    let (reply, rx) = mpsc::channel();
    sender()?
        .send(Request { path: path.to_path_buf(), stamp, page, dpi, background, reply })
        .map_err(|_| "PdfPage_H の裏のスレッドが止まっている".to_string())?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(format!("{} 秒以内に描き終わらなかった", timeout.as_secs()))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("PdfPage_H の裏のスレッドが途中で止まった".into())
        }
    }
}

/// 裏のスレッドを止めて終わりを待つ（プラグイン本体の Drop から呼ぶ）
pub fn shutdown() {
    STOPPED.store(true, std::sync::atomic::Ordering::Release);
    let worker = WORKER.lock().ok().and_then(|mut g| g.take());
    if let Some(Worker { tx, handle }) = worker {
        // 送り口を閉じると、今の 1 件を描き終えたところで run のループが抜ける
        drop(tx);
        let _ = handle.join();
    }
}

/// 描いたページを見分ける鍵（ページは丸めて寄せた後の 0 始まり）
#[derive(Debug, Clone, PartialEq)]
struct PageKey {
    path: PathBuf,
    stamp: FileStamp,
    index: u32,
    dpi: u32,
    background: Option<(u8, u8, u8)>,
}

type Docs = Vec<((PathBuf, FileStamp), PdfDocument)>;

fn run(rx: mpsc::Receiver<Request>) {
    let initialized = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
    {
        let mut docs: Docs = Vec::new();
        // 先に描いておいた次のページ（1 枚だけ）
        let mut ahead: Option<(PageKey, Rendered)> = None;
        let mut pending: Option<Request> = None;
        loop {
            let req = match pending.take() {
                Some(req) => req,
                None => match rx.recv() {
                    Ok(req) => req,
                    Err(_) => break,
                },
            };
            let result = if initialized {
                serve(&mut docs, &mut ahead, &req).map_err(|e| describe(&req.path, &e))
            } else {
                Err("WinRT を初期化できない".into())
            };
            let next = result.as_ref().ok().map(|(_, key, count)| (key.clone(), *count));
            // 待ち切れずに諦めた側は受け口を捨てている。送れなくても構わない
            let _ = req.reply.send(result.map(|(image, _, _)| image));
            // 次の頼みが来ていなければ、次のページを先に描いておく（再生でページが変わるフレームを軽くする）
            match rx.try_recv() {
                Ok(req) => pending = Some(req),
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {
                    if let Some((key, count)) = next {
                        prefetch(&mut docs, &mut ahead, key, count);
                    }
                }
            }
        }
        // PdfDocument は RoUninitialize より先に捨てる
    }
    if initialized {
        unsafe { RoUninitialize() };
    }
}

/// 頼まれたページを返す。先に描いてあればそれを渡す
fn serve(
    docs: &mut Docs,
    ahead: &mut Option<(PageKey, Rendered)>,
    req: &Request,
) -> windows::core::Result<(Rendered, PageKey, u32)> {
    let doc = open(docs, &req.path, req.stamp)?;
    let count = doc.PageCount()?;
    let key = PageKey {
        path: req.path.clone(),
        stamp: req.stamp,
        index: page_number(req.page as f64, count) - 1,
        dpi: req.dpi,
        background: req.background,
    };
    if ahead.as_ref().is_some_and(|(k, _)| *k == key) {
        let (_, image) = ahead.take().unwrap();
        return Ok((image, key, count));
    }
    let image = draw(doc, &key)?;
    Ok((image, key, count))
}

/// 次のページを描いて取っておく。失敗しても何もしない（頼まれたときに描き直して知らせる）
fn prefetch(docs: &mut Docs, ahead: &mut Option<(PageKey, Rendered)>, key: PageKey, count: u32) {
    if key.index + 1 >= count {
        return;
    }
    let next = PageKey { index: key.index + 1, ..key };
    // 同じ描き方（解像度・背景まで）で描いてあるときだけ描き直さない
    if ahead.as_ref().is_some_and(|(k, _)| *k == next) {
        return;
    }
    let image = open(docs, &next.path, next.stamp).and_then(|doc| draw(doc, &next));
    *ahead = image.ok().map(|image| (next, image));
}

fn describe(path: &Path, e: &windows::core::Error) -> String {
    let detail = e.message();
    let detail = if detail.trim().is_empty() { format!("HRESULT {:#010x}", e.code().0) } else { detail };
    format!("{} を描けない（パスワード付きか壊れた PDF の可能性）: {detail}", path.display())
}

fn open<'a>(
    docs: &'a mut Docs,
    path: &Path,
    stamp: FileStamp,
) -> windows::core::Result<&'a PdfDocument> {
    let key = (path.to_path_buf(), stamp);
    if let Some(i) = docs.iter().position(|(k, _)| *k == key) {
        // 最近使ったものを末尾へ
        let entry = docs.remove(i);
        docs.push(entry);
    } else {
        // 同じファイルの古い版（書き換えられる前）は捨てる
        docs.retain(|((p, _), _)| p != path);
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
        let doc = PdfDocument::LoadFromFileAsync(&file)?.join()?;
        if docs.len() >= OPEN_DOCS {
            docs.remove(0);
        }
        docs.push((key, doc));
    }
    Ok(&docs.last().unwrap().1)
}

fn draw(doc: &PdfDocument, key: &PageKey) -> windows::core::Result<Rendered> {
    let page = doc.GetPage(key.index)?;
    // Size は回転を含み、単位は DIP（720 x 405 pt のページで 960 x 540。プローブで確かめた）
    let size = page.Size()?;
    let (width, height) = target_size(size.Width, size.Height, key.dpi as f64);

    let options = PdfPageRenderOptions::new()?;
    options.SetDestinationWidth(width)?;
    options.SetDestinationHeight(height)?;
    let background = match key.background {
        Some((r, g, b)) => Color { A: 255, R: r, G: g, B: b },
        // A = 0 で紙の白も描かれない（プローブで確かめた）
        None => Color { A: 0, R: 0, G: 0, B: 0 },
    };
    options.SetBackgroundColor(background)?;

    let stream = InMemoryRandomAccessStream::new()?;
    page.RenderWithOptionsToStreamAsync(&stream, &options)?.join()?;
    stream.Seek(0)?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let pixels = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Rgba8,
            BitmapAlphaMode::Straight,
            &BitmapTransform::new()?,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?;
    let rgba = pixels.DetachPixelData()?.to_vec();
    let (w, h) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    if rgba.len() != (w as usize) * (h as usize) * 4 {
        return Err(windows::core::Error::new(
            windows::core::HRESULT(0x8000_FFFFu32 as i32),
            format!("画素の数が合わない（{w}x{h} に {} バイト）", rgba.len()),
        ));
    }
    Ok(Rendered { width: w, height: h, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手元の PDF を描く。`PDF_PAGE_H_TEST_PDF` に PDF のパスを入れて `cargo test -- --ignored` で走らせる
    #[test]
    #[ignore]
    fn renders_local_pdf() {
        let path = PathBuf::from(std::env::var("PDF_PAGE_H_TEST_PDF").expect("PDF_PAGE_H_TEST_PDF"));
        let stamp = FileStamp::of(&path).unwrap();
        let t = std::time::Instant::now();
        let white = render(&path, stamp, 1, 192, Some((255, 255, 255)), Duration::from_secs(10)).unwrap();
        println!("1 ページ目 {}x{} {:.1}ms", white.width, white.height, t.elapsed().as_secs_f64() * 1e3);
        assert!(white.rgba.chunks(4).all(|p| p[3] == 255), "背景ありは不透明");
        let clear = render(&path, stamp, 1, 192, None, Duration::from_secs(10)).unwrap();
        assert!(clear.rgba.chunks(4).any(|p| p[3] == 0), "背景なしは透明の所がある");
        // 次のページは裏で先に描いてあるので速い（1 ページ目を描いた後に先読みが終わるのを待つ）
        std::thread::sleep(Duration::from_millis(500));
        let t = std::time::Instant::now();
        let second = render(&path, stamp, 2, 192, None, Duration::from_secs(10)).unwrap();
        println!("2 ページ目（先読み） {}x{} {:.1}ms", second.width, second.height, t.elapsed().as_secs_f64() * 1e3);
        let t = std::time::Instant::now();
        render(&path, stamp, 5, 192, None, Duration::from_secs(10)).unwrap();
        println!("5 ページ目（飛ばした） {:.1}ms", t.elapsed().as_secs_f64() * 1e3);
        // ページ数を超えたら最後のページ（落ちない）
        let last = render(&path, stamp, 9999, 96, None, Duration::from_secs(10)).unwrap();
        assert!(last.width > 0);
        shutdown();
        assert!(render(&path, stamp, 1, 96, None, Duration::from_secs(1)).is_err(), "止めた後は描かない");
    }
}

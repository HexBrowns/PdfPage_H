//! PdfPage_H: PDF のページを画像のオブジェクトとして出す（カスタムオブジェクト「PDF_H」）。
//!
//! - 描くのは Windows 標準の `Windows.Data.Pdf`（外部 DLL を同梱しない）。裏のスレッドで描く（`worker.rs`）
//! - 描いた画像は本体の画像キャッシュに置き、同じページが続く間は描き直さない
//! - `.pdf` をタイムラインに落とすと、落とした位置にこのオブジェクトを置く

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use aviutl2::anyhow::{self, Context};
use aviutl2::filter::{FilterConfigItemSliceExt, FilterConfigItems};
use aviutl2::tracing;

pub mod render;
pub mod worker;

use render::{FileStamp, RenderKey};

/// カスタムオブジェクトの名前（`.aup2` の `effect.name` に入る。変えると保存済みのプロジェクトから見つからなくなる）
const OBJECT_NAME: &str = "PDF_H";

/// 1 ページを描き終えるまで待つ上限。超えたら透明を出す（書き出しで抜けたフレームを作らないよう長めに取る）
const RENDER_TIMEOUT: Duration = Duration::from_secs(10);

#[aviutl2::plugin(GenericPlugin)]
struct PdfPageH {
    object: aviutl2::generic::SubPlugin<PdfObject>,
}

static EDIT_HANDLE: aviutl2::generic::GlobalEditHandle = aviutl2::generic::GlobalEditHandle::new();

impl aviutl2::generic::GenericPlugin for PdfPageH {
    fn new(info: aviutl2::AviUtl2Info) -> aviutl2::AnyResult<Self> {
        let _ = aviutl2::tracing_subscriber::fmt()
            .with_max_level(if cfg!(debug_assertions) { tracing::Level::DEBUG } else { tracing::Level::INFO })
            .event_format(aviutl2::logger::AviUtl2Formatter)
            .with_writer(aviutl2::logger::AviUtl2LogWriter)
            .try_init();
        Ok(Self { object: aviutl2::generic::SubPlugin::new_filter_plugin(&info)? })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "PdfPage_H".to_string(),
            information: format!("PDF のページを画像のオブジェクトとして出す / v{} by HexBrowns", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        registry.register_filter_plugin(&self.object);
        EDIT_HANDLE.init(registry.create_edit_handle());
        let filters = aviutl2::file_filters! { "PDF" => ["pdf"] };
        registry.register_file_drop_handler("PdfPage_H", &filters, |file| {
            if let Err(e) = place_dropped(&file) {
                tracing::warn!("PDF_H: {} を置けなかった: {e:#}", file.display());
            }
        });
    }
}

impl Drop for PdfPageH {
    fn drop(&mut self) {
        worker::shutdown();
    }
}

/// 落とした位置に PDF_H を置き、ファイルを設定する（Ctrl+Z 1 回で消える）
fn place_dropped(file: &std::path::Path) -> anyhow::Result<()> {
    let path = file.to_str().context("パスを文字列にできない")?;
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or(OBJECT_NAME);
    let (layer, frame) = EDIT_HANDLE
        .call_edit_section(|e| {
            let position = e.get_mouse_layer_frame()?;
            let (layer, frame) = match position {
                Some(p) => (p.layer, p.frame),
                None => (e.info.layer, e.info.frame),
            };
            let mut alias = aviutl2::alias::Table::new();
            let mut object = aviutl2::alias::Table::new();
            object.insert_value("name", name);
            let mut effect = aviutl2::alias::Table::new();
            effect.insert_value("effect.name", OBJECT_NAME);
            effect.insert_value("ファイル", path);
            object.insert_table("0", effect);
            alias.insert_table("Object", object);
            e.create_object_from_alias(&alias.to_string(), layer, frame, 0)?;
            anyhow::Ok((layer, frame))
        })
        .map_err(anyhow::Error::from)
        .flatten()?;
    tracing::info!("PDF_H: {name} を置いた（レイヤー {layer}、フレーム {frame}）");
    Ok(())
}

#[aviutl2::plugin(FilterPlugin)]
struct PdfObject;

#[aviutl2::filter::filter_config_items]
struct PdfConfig {
    #[file(name = "ファイル", filters = { "PDF" => ["pdf"] })]
    file: Option<PathBuf>,
    #[track(name = "ページ", range = 1.0..=9999.0, default = 1.0, step = 1.0)]
    page: f64,
    #[track(name = "解像度", range = 72.0..=600.0, default = 192.0, step = 1.0)]
    dpi: f64,
    #[check(name = "背景", default = true)]
    background: bool,
    #[color(name = "背景色", default = 0xffffff)]
    background_color: aviutl2::filter::FilterConfigColorValue,
}

static REPORTED: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// 同じ失敗はログに 1 回だけ出す（描画は毎フレーム呼ばれる）
fn warn_once(key: &impl Hash, message: &str) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (key, message).hash(&mut hasher);
    let first = REPORTED.lock().map(|mut set| set.insert(hasher.finish())).unwrap_or(true);
    if first {
        tracing::warn!("PDF_H: {message}");
    }
}

fn set_blank(video: &mut aviutl2::filter::FilterProcVideo<()>) {
    video.set_image_data(&[0u8; 4], 1, 1);
}

impl aviutl2::filter::FilterPlugin for PdfObject {
    type Userdata = ();

    fn new(_info: aviutl2::AviUtl2Info) -> aviutl2::AnyResult<Self> {
        Ok(Self)
    }

    fn plugin_info(&self) -> aviutl2::filter::FilterPluginTable {
        aviutl2::filter::FilterPluginTable {
            name: OBJECT_NAME.into(),
            label: None,
            flags: aviutl2::bitflag!(aviutl2::filter::FilterPluginFlags { video: true, input: true }),
            information: format!("PDF のページを描く（Windows.Data.Pdf） / v{} by HexBrowns", env!("CARGO_PKG_VERSION")),
            config_items: PdfConfig::to_config_items(),
        }
    }

    fn proc_video(
        &self,
        config: &[aviutl2::filter::FilterConfigItem],
        video: &mut aviutl2::filter::FilterProcVideo<()>,
    ) -> aviutl2::AnyResult<()> {
        // 失敗しても Err を返さない（本体が毎フレーム ERROR を出す）。透明を出して 1 回だけ記録する
        let config = config.to_struct::<PdfConfig>();
        let Some(path) = config.file.filter(|p| !p.as_os_str().is_empty()) else {
            set_blank(video);
            return Ok(());
        };
        let Some(stamp) = FileStamp::of(&path) else {
            warn_once(&path, &format!("{} が見つからない", path.display()));
            set_blank(video);
            return Ok(());
        };
        let key = RenderKey {
            path,
            stamp,
            page: config.page.round().clamp(1.0, 9999.0) as u32,
            dpi: config.dpi.round().clamp(72.0, 600.0) as u32,
            background: config.background.then(|| config.background_color.to_rgb()),
        };
        let name = key.cache_name();
        let handle = &aviutl2::cache::GLOBAL_CACHE_HANDLE;
        if let Ok(Some(entry)) = aviutl2::cache::get_image_cache(handle, &name) {
            video.set_image_data(entry.as_u8_slice(), entry.width() as _, entry.height() as _);
            return Ok(());
        }
        match worker::render(&key.path, key.stamp, key.page, key.dpi, key.background, RENDER_TIMEOUT) {
            Ok(image) => {
                match aviutl2::cache::create_image_cache(handle, &name, image.width as _, image.height as _) {
                    Ok(mut entry) if entry.as_u8_slice().len() == image.rgba.len() => {
                        entry.as_u8_slice_mut().copy_from_slice(&image.rgba);
                        video.set_image_data(entry.as_u8_slice(), entry.width() as _, entry.height() as _);
                    }
                    // キャッシュに置けなくても絵は出す（次のフレームで描き直す）
                    _ => video.set_image_data(&image.rgba, image.width, image.height),
                }
            }
            Err(message) => {
                warn_once(&key, &message);
                set_blank(video);
            }
        }
        Ok(())
    }
}

aviutl2::register_generic_plugin!(PdfPageH);

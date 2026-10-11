# PdfPage_H（開発メモ）

PDF のページを画像のオブジェクトとして出す AviUtl2 の汎用プラグイン（`.aux2`）。使い方は `自作スクリプトマニュアル/PdfPage_H_Manual.md`、
仕様と決定は `AI/specifications/20261009_PdfPage_H_spec.md`。

- aviutl2-rs 0.49（最小サポートは本体 2.1.13）、`windows` 0.62（`Data_Pdf` `Graphics_Imaging` `Storage` `Storage_Streams`）
- 1 つの `.aux2` に、カスタムオブジェクト「PDF_H」（フィルタプラグイン、`FLAG_VIDEO | FLAG_INPUT`）と `.pdf` のドロップの処理を同居させる（svg_h と同じ形）

## 作り

| ファイル | 中身 |
|---|---|
| `src/lib.rs` | 登録（カスタムオブジェクト・ドロップ）、`proc_video`（本体の画像キャッシュに当たればそれを返し、外れたら裏のスレッドに頼んで最大 10 秒待つ） |
| `src/worker.rs` | PDF 専用の裏のスレッド。WinRT を MTA で初期化し、`PdfDocument` を 4 本まで開いたまま持つ。描き終えて次の頼みが無ければ、次のページを先に描いて 1 枚取っておく。プラグインの `Drop` で止めて終わりを待つ |
| `src/render.rs` | PDF に触らない計算（画素数・ページの丸め・キャッシュの鍵）。`cargo test` |
| `examples/render_page.rs` | 机上の検証用。プラグインと同じ描き方で 1 ページを生の RGBA に書く |
| `tests/make_test_pdf.py` | 試験用の 10 ページの PDF（外部ライブラリなし）。検証の節 `AI/tools/debug_items/pdf.py` も使う |
| `tests/verify_judge.py` | 検証の節の判定が、正しい絵で通り、取り違えを再現した絵で落ちることを確かめる（開発用リポジトリの検証の道具に頼るので、公開側には出さない） |

## 確かめたこと（2026-10-11）

- **パッケージ無しのデスクトップのプロセスから `Windows.Data.Pdf` が動く**（単体のプローブと本番の本体 2.1.13）
- `PdfPage.Size` は DIP（1/96 インチ）で、回転を含む。720 x 405 pt のページで 960 x 540
- `PdfPageRenderOptions.BackgroundColor` を `A = 0` にすると紙の白も描かれない（透明）
- `BitmapDecoder.GetPixelDataTransformedAsync(Rgba8, Straight, …)` の画素は本体の `PIXEL_RGBA`（ストレート）にそのまま渡せる（実機の検証 PDF-03 で半透明の色が暗くならない）
- 速度（本番、FHD）: 試験用の 16:9 は 1 ページ 20〜35ms、日本語・画像入りの A4（`Script/SeiSa/README.pdf`）は 95〜180ms。先に描いた次のページは 0.1ms 未満

## テスト

```bash
cargo test                                    # 計算
cargo build --release --example render_page
python tests/verify_judge.py                  # 検証の節の判定（実際に描く。開発用リポジトリだけ）
```

`cargo test -- --ignored` は環境変数 `PDF_PAGE_H_TEST_PDF` の PDF を描く（先読みの速さも出す）。

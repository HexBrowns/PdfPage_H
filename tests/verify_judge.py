"""PDF 節（`AI/tools/debug_items/pdf.py`）の判定が、正しい絵で通り、壊れた絵で落ちることを机上で確かめる。

絵はプラグインと同じ描き方（`examples/render_page.rs` → `worker::render`）で作り、
生成プロジェクトの並び（透明の地に、左右 ±480 か中央）に置いて `judge_shots.judge_one` に通す。
壊れた絵は、実装の取り違えを 1 つずつ再現したもの（ページを見ない・背景を見ない・乗算済みとストレートの取り違えなど）。

使い方（PdfPage_H のフォルダで）:
    cargo build --release --example render_page
    python tests/verify_judge.py
"""
import io
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image

HERE = Path(__file__).resolve().parent
PLUGIN = HERE.parent
ROOT = PLUGIN.parents[2]
sys.path.insert(0, str(ROOT / 'AI' / 'tools'))
import judge_shots  # noqa: E402
from debug_items import pdf  # noqa: E402

EXE = PLUGIN / 'target' / 'release' / 'examples' / 'render_page.exe'
OUT = io.open(1, 'w', encoding='utf-8', closefd=False)
TMP = Path(tempfile.mkdtemp(prefix='pdf_page_h_'))


def log(msg):
    OUT.write(msg + '\n')
    OUT.flush()


def asset(rel):
    p = TMP / Path(rel).name
    if not p.exists():
        p.write_bytes(pdf.ASSETS[rel])
    return p


def render(page=1, dpi=86, bg='ffffff', rel=pdf.TEST):
    """プラグインの描き方で描いた RGBA（ストレート、0..1）"""
    out = TMP / ('p%d_%d_%s.raw' % (page, dpi, bg))
    r = subprocess.run([str(EXE), str(asset(rel)), str(page), str(dpi), bg, str(out)], capture_output=True)
    if r.returncode != 0:
        return None
    b = out.read_bytes()
    w, h = int.from_bytes(b[0:4], 'little'), int.from_bytes(b[4:8], 'little')
    return np.frombuffer(b[8:], np.uint8).reshape(h, w, 4).astype(np.float32) / 255.0


def blank():
    return np.zeros((1, 1, 4), np.float32)


def place(canvas, img, cx):
    """ストレートの img を乗算済みにして、中心 (cx, 540) に重ねる（画面の外は切る）"""
    h, w = img.shape[:2]
    y0, x0 = 540 - h // 2, cx - w // 2
    pm = img.copy()
    pm[..., :3] *= pm[..., 3:4]
    ys, xs = max(0, y0), max(0, x0)
    ye, xe = min(1080, y0 + h), min(1920, x0 + w)
    src = pm[ys - y0:ye - y0, xs - x0:xe - x0]
    dst = canvas[ys:ye, xs:xe]
    a = src[..., 3:4]
    dst[...] = src + dst * (1 - a)


def shot(left=None, right=None, center=None):
    canvas = np.zeros((1080, 1920, 4), np.float32)
    for img, cx in ((left, 480), (right, 1440), (center, 960)):
        if img is not None:
            place(canvas, img, cx)
    p = tempfile.mktemp(suffix='.png', dir=str(TMP))
    Image.fromarray(np.floor(np.clip(canvas, 0, 1) * 255 + 0.5).astype(np.uint8), 'RGBA').save(p)
    return p


def judge(path, iid):
    return judge_shots.judge_one(path, pdf.JUDGE[iid], 40)


def ok(rows):
    return all(r[4] for r in rows)


def premult_as_straight(img):
    """乗算済みの値をストレートとして渡した絵（半透明の色が暗くなる）"""
    out = img.copy()
    out[..., :3] *= out[..., 3:4]
    return out


FAILS = []


def check(iid, good, bads):
    rows = judge(good, iid)
    passed = ok(rows)
    detail = ' / '.join('%s=%.3f' % (r[0].split('.')[-1], r[3]) for r in rows)
    log('%s %s 正しい絵: %s' % ('ok ' if passed else 'NG ', iid, detail))
    if not passed:
        FAILS.append(iid)
    for name, bad in bads:
        caught = not ok(judge(bad, iid))
        log('%s %s 壊れた絵（%s）で落ちる' % ('ok ' if caught else 'NG ', iid, name))
        if not caught:
            FAILS.append('%s/%s' % (iid, name))


def main():
    if not EXE.exists():
        log('[ERROR] %s が無い。先に cargo build --release --example render_page' % EXE)
        return 1
    p1, p2, p3 = render(1), render(2), render(3)
    check('PDF-00', shot(p1, p2), [('ページを見ない', shot(p1, p1))])
    p10 = render(10, 72)
    check('PDF-01', shot(p10, render(9999, 72)),
          [('超えたら描けない', shot(p10, blank())), ('超えたら 1 ページ目', shot(p10, render(1, 72)))])
    check('PDF-02', shot(render(1, 72), render(9, 72)),
          [('解像度を見ない（96）', shot(render(1, 96), render(9, 96))), ('縦横を取り違える', shot(render(1, 72), render(10, 72)))])
    clear = render(1, 86, 'none')
    check('PDF-03', shot(p1, clear),
          [('背景 OFF を見ない', shot(p1, p1)), ('乗算済みをストレートとして渡す', shot(p1, premult_as_straight(clear)))])
    check('PDF-04', shot(p1, render(1, 86, '202020')), [('背景色を見ない', shot(p1, p1))])
    check('PDF-05', shot(center=render(1, 144)), [('解像度を見ない（192）', shot(center=render(1, 192)))])
    broken = render(rel=pdf.BROKEN)
    log('     壊れた PDF を描こうとした結果: %s' % ('描けた（想定外）' if broken is not None else '失敗（想定どおり）'))
    for iid in ('PDF-06', 'PDF-07'):
        check(iid, shot(p1, blank()), [('何か描いてしまう', shot(p1, p1))])
    frames = [shot(p1), shot(p2), shot(p3)]
    for k, iid in enumerate(('PDF-08', 'PDF-08#1', 'PDF-08#2')):
        bad = [] if k == 0 else [('ページが動かない', frames[0])]
        check(iid, frames[k], bad)
    log('')
    log('[%s] 失敗 %d 件%s' % ('ERROR' if FAILS else 'INFO', len(FAILS), ('（%s）' % ', '.join(FAILS)) if FAILS else ''))
    return 1 if FAILS else 0


if __name__ == '__main__':
    sys.exit(main())

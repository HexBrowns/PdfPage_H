"""PdfPage_H の試験用の PDF を書く（外部ライブラリなし）。使い方: python make_test_pdf.py <出力.pdf>

10 ページ: 1〜8 は 16:9（720 x 405 pt = 10 x 5.625 インチ）、9 は A4 縦、10 は A4 横。
各ページに大きなページ番号（Helvetica、埋め込みなし）、色の帯、細い線、半透明の重なりを描く。
"""
import sys

COLORS = [(0.85, 0.2, 0.2), (0.2, 0.6, 0.25), (0.2, 0.35, 0.85), (0.9, 0.6, 0.1),
          (0.6, 0.2, 0.7), (0.1, 0.65, 0.7), (0.5, 0.5, 0.5), (0.8, 0.3, 0.55),
          (0.3, 0.3, 0.3), (0.15, 0.45, 0.3)]


def page_size(i):
    if i == 9:
        return 595.0, 842.0
    if i == 10:
        return 842.0, 595.0
    return 720.0, 405.0


def content(i):
    w, h = page_size(i)
    r, g, b = COLORS[i - 1]
    s = []
    # 左の色の帯（ページ全体の高さ）
    s.append('%.3f %.3f %.3f rg 0 0 %.1f %.1f re f' % (r, g, b, w * 0.12, h))
    # 半透明の円の代わりに半透明の四角（ExtGState /GS0 で ca 0.5）
    s.append('q /GS0 gs 0 0 1 rg %.1f %.1f %.1f %.1f re f Q' % (w * 0.55, h * 0.1, w * 0.3, h * 0.3))
    s.append('q /GS0 gs 1 0 0 rg %.1f %.1f %.1f %.1f re f Q' % (w * 0.65, h * 0.2, w * 0.3, h * 0.3))
    # 細い線（0.5pt）を 10 本
    s.append('0 0 0 RG 0.5 w')
    for k in range(10):
        x = w * 0.2 + k * 6
        s.append('%.1f %.1f m %.1f %.1f l S' % (x, h * 0.1, x, h * 0.45))
    # ページ番号
    s.append('BT /F1 %d Tf 0 0 0 rg %.1f %.1f Td (Page %d) Tj ET' % (int(min(w, h * 1.6) * 0.15), w * 0.2, h * 0.6, i))
    s.append('BT /F1 14 Tf 0.2 0.2 0.2 rg %.1f %.1f Td (PdfPage_H test %.0fx%.0f pt) Tj ET' % (w * 0.2, h * 0.52, w, h))
    return '\n'.join(s).encode('ascii')


def pdf_bytes(n=10):
    """試験用の PDF のバイト列（毎回同じ）"""
    objs = []  # 1-based

    def add(body):
        objs.append(body)
        return len(objs)

    catalog = add(None)
    pages = add(None)
    font = add(b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>')
    gs = add(b'<< /Type /ExtGState /ca 0.5 /CA 0.5 >>')
    kids = []
    for i in range(1, n + 1):
        w, h = page_size(i)
        c = content(i)
        cid = add(b'<< /Length %d >>\nstream\n' % len(c) + c + b'\nendstream')
        pid = add(('<< /Type /Page /Parent %d 0 R /MediaBox [0 0 %.0f %.0f] /Contents %d 0 R '
                   '/Resources << /Font << /F1 %d 0 R >> /ExtGState << /GS0 %d 0 R >> >> >>'
                   % (pages, w, h, cid, font, gs)).encode('ascii'))
        kids.append(pid)
    objs[catalog - 1] = ('<< /Type /Catalog /Pages %d 0 R >>' % pages).encode('ascii')
    objs[pages - 1] = ('<< /Type /Pages /Kids [%s] /Count %d >>'
                       % (' '.join('%d 0 R' % k for k in kids), len(kids))).encode('ascii')
    out = bytearray(b'%PDF-1.4\n%\xe2\xe3\xcf\xd3\n')
    offs = []
    for k, body in enumerate(objs, 1):
        offs.append(len(out))
        out += b'%d 0 obj\n' % k + body + b'\nendobj\n'
    xref = len(out)
    out += b'xref\n0 %d\n0000000000 65535 f \n' % (len(objs) + 1)
    for o in offs:
        out += b'%010d 00000 n \n' % o
    out += b'trailer\n<< /Size %d /Root %d 0 R >>\nstartxref\n%d\n%%%%EOF\n' % (len(objs) + 1, catalog, xref)
    return bytes(out)


def build(path, n=10):
    open(path, 'wb').write(pdf_bytes(n))


if __name__ == '__main__':
    build(sys.argv[1])
    print('ok', sys.argv[1])

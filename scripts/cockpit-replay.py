#!/usr/bin/env python3
"""Replay clicks and typing on the Cockpit's own page, without the app.

`kova render` writes the page (the app's shell and script); a headless Chrome
plays real DOM events on it and this script reads what the page would have
sent to Kova. It replays the case of 14/11/2026: a card is filed, then the
user answers the card whose screenshots are under his eye. Before the fix
the only field in sight was the neighbour's and the text left with the
neighbour's No go.

  scripts/cockpit-replay.py [--kova <binary>] [--list <queue json>] [--keep <dir>]

Exit 0 when every check holds, 1 when one fails, 2 when it could not run.
The queue names three cards, in the page's order: C (screenshots, no
draft), A (the one filed first, opened), B (a draft).
"""
import argparse, html, json, os, pathlib, re, subprocess, sys, tempfile

HERE = pathlib.Path(__file__).resolve().parent
MARK = '<script nonce="render">window.kova && window.kova.update('

SCENARIO = r"""
const [C, A, B] = DATA.order;
const state = { open: new Set(), gone: new Set(), filed: null, tick: 0 };
function listHtml() {
  const body = DATA.order.filter(k => !state.gone.has(k) || state.filed === k)
    .map(k => state.filed === k ? DATA.filed[k] : ((state.open.has(k) && DATA.opened[k]) || DATA.folded[k])).join('');
  return DATA.frame[0] + body + DATA.frame[1] + '<!--' + (state.tick++) + '-->';
}
function push() { window.kova.update({ list: listHtml() }); }
// Kova's side: a card filed becomes the one receipt, in its place.
function drain() {
  const got = window.posted.splice(0).filter(m => m.type === 'act');
  for (const m of got) {
    if (m.act === 'expand') { state.open.has(m.key) ? state.open.delete(m.key) : state.open.add(m.key); }
    if (m.act === 'reply' && ['go', 'nogo', 'seen'].includes(m.arg)) { state.gone.add(m.key); state.filed = m.key; }
  }
  push();
  return got.filter(m => m.act !== 'expand').map(m => ({ key: m.key, arg: m.arg, text: m.text || '' }));
}
const card = k => document.querySelector('.card[data-key="' + k + '"]');
const wait = ms => new Promise(r => setTimeout(r, ms));
function center(el) { const r = el.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }
function under(x, y) {
  const el = document.elementFromPoint(x, y), c = el && el.closest('.card'), b = el && el.closest('button, textarea');
  return { card: c ? c.dataset.key : null, receipt: !!(c && c.classList.contains('filed')), control: b ? b.tagName.toLowerCase() : null };
}
const checks = [];
const check = (name, ok, detail) => checks.push({ name, ok: !!ok, detail: detail === undefined ? null : detail });
function done() { document.getElementById('out').textContent = JSON.stringify(checks); }
window.addEventListener('load', async () => { try {
  state.open = new Set([A]); push(); await wait(60);
  const go = card(A).querySelector('[data-arg=go]');
  go.scrollIntoView({ block: 'center' }); await wait(40);
  const [x, y] = center(go);
  const clickAt = (px, py) => document.elementFromPoint(px, py).dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1, clientX: px, clientY: py }));
  clickAt(x, y);
  check('a filed card stays in place, out of reach, until its receipt comes',
    card(A) && getComputedStyle(card(A)).display !== 'none' && card(A).classList.contains('going'));
  const sent = window.posted.length;
  card(B).querySelector('[data-arg=nogo]').click();
  card(C).querySelector('[data-arg=seen]').click();
  check('a button clicked right after a filing does nothing', window.posted.length === sent);
  const first = drain();
  check('the Go went to the card clicked, alone', first.length === 1 && first[0].key === A && first[0].arg === 'go', first);
  await wait(60);
  check('the filed card is its receipt, in the list', card(A) && card(A).classList.contains('filed') && /Go noté sur cette carte/.test(card(A).textContent));
  // The cards below moved up: whatever is under the pointer now, a click
  // there does nothing until the pointer has moved.
  await wait(800);
  // (Opening or folding a card is not a gesture: it is not held.)
  const acts = () => window.posted.filter(m => m.type === 'act' && m.act !== 'expand').length;
  const u = under(x, y), n = acts();
  clickAt(x, y);
  card(B).querySelector('[data-arg=nogo]').dispatchEvent(new MouseEvent('click', { bubbles: true, detail: 1, clientX: x, clientY: y }));
  check('a click where the pointer stayed never acts on what slid under it', acts() === n, u);
  window.posted.splice(0).forEach(m => { if (m.type === 'act' && m.act === 'expand') { /* dropped: the replay keeps its cards as they are */ } });
  document.dispatchEvent(new MouseEvent('mousemove', { bubbles: true, clientX: x + 40, clientY: y + 40 }));
  const field = card(C).querySelector('textarea.rz-t'), label = c => (card(c).querySelector('.rz .lbl') || {}).textContent || '';
  check('a card that shows screenshots shows its own field, folded', field && !card(C).classList.contains('open'));
  check('each field names its card', label(C).includes(card(C).querySelector('.t').textContent) && label(B).includes(card(B).querySelector('.t').textContent), [label(C), label(B)]);
  field.focus(); field.value = 'the pale blue is wrong'; field.dispatchEvent(new Event('input', { bubbles: true }));
  push();
  check('a refresh while typing keeps the text in its card', card(C).querySelector('textarea.rz-t').value === 'the pale blue is wrong' && card(B).querySelector('textarea.rz-t').value === '');
  card(C).querySelector('[data-arg=nogo]').click();
  const second = drain();
  check("the text typed under a card's screenshots leaves with that card's No go",
    second.length === 1 && second[0].key === C && second[0].arg === 'nogo' && second[0].text === 'the pale blue is wrong', second);
  await wait(60);
  check('one receipt at a time, the last gesture', document.querySelectorAll('.card.filed').length === 1 && card(C).classList.contains('filed') && !card(A));
  check('the neighbour was not touched', card(B) && !card(B).classList.contains('filed') && card(B).querySelector('textarea.rz-t').value === '');
  done();
} catch (e) { check('the replay ran', false, String(e && e.stack || e)); done(); }
});
"""


def split(page):
    i = page.rindex(MARK)
    upd, _ = json.JSONDecoder().raw_decode(page[i + len(MARK):].replace("<\\/", "</"))
    return page[:i], upd


def cards(list_html):
    return {m.group(1): m.group(0) for m in re.finditer(r'<article class="card[^"]*" data-key="([^"]+)">.*?</article>', list_html, re.S)}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--kova", default=os.environ.get("KOVA", "kova"))
    ap.add_argument("--list", default=str(HERE.parent / "tests/fixtures/attention/neighbours.json"))
    ap.add_argument("--keep", help="write the pages here instead of a temporary folder")
    ap.add_argument("--chrome", default=os.environ.get("CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"))
    a = ap.parse_args()
    if not os.access(a.chrome, os.X_OK):
        print(f"cockpit-replay: no Chrome at {a.chrome} (set CHROME)", file=sys.stderr)
        return 2
    ids = [e["id"] for e in json.load(open(a.list))]
    tmp = pathlib.Path(a.keep) if a.keep else pathlib.Path(tempfile.mkdtemp(prefix="kova-replay-"))
    tmp.mkdir(parents=True, exist_ok=True)

    def render(name, *more):
        out = tmp / name
        r = subprocess.run([a.kova, "render", "cockpit", "--no-config", "--list", a.list, "--out", str(out), *more], capture_output=True, text=True)
        if r.returncode != 0:
            print(f"cockpit-replay: kova render failed: {r.stderr.strip()}", file=sys.stderr)
            sys.exit(2)
        return out

    shell, upd = split((render("folded") / "default.html").read_text())
    folded = cards(upd["list"])
    order = list(folded)
    opened, filed = {}, {}
    for i in ids:
        opened.update(cards(split((render(f"open-{i}", "--open", i) / "card.html").read_text())[1]["list"]))
        for word in ("go", "nogo"):
            got = cards(split((render(f"filed-{i}-{word}", "--filed", f"{i}={word}:the pale blue is wrong" if word == "nogo" else f"{i}={word}") / "default.html").read_text())[1]["list"])
            key = f"attention:{i}"
            if 'class="card filed"' in got.get(key, "") and (word == "go") == (key == order[1]):
                filed[key] = got[key]
    first = upd["list"].index(folded[order[0]])
    last = upd["list"].index(folded[order[-1]]) + len(folded[order[-1]])
    data = {"order": order, "folded": folded, "opened": opened, "filed": filed, "frame": [upd["list"][:first], upd["list"][last:]]}
    stub = '<script nonce="render">window.posted=[];window.webkit={messageHandlers:new Proxy({},{get:()=>({postMessage:m=>window.posted.push(JSON.parse(m))})})};</script>'
    page = stub + shell + '<pre id="out" hidden></pre><script nonce="render">const DATA=' + json.dumps(data).replace("</", "<\\/") + ";" + SCENARIO + "</script></body></html>"
    (tmp / "replay.html").write_text(page)
    r = subprocess.run([a.chrome, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--window-size=1000,900", "--virtual-time-budget=30000", "--dump-dom", f"file://{tmp / 'replay.html'}"], capture_output=True, text=True)
    m = re.search(r'<pre id="out"[^>]*>(.*?)</pre>', r.stdout, re.S)
    if not m or not m.group(1):
        print("cockpit-replay: the page gave no result", file=sys.stderr)
        return 2
    checks = json.loads(html.unescape(m.group(1)))
    for c in checks:
        print(("ok    " if c["ok"] else "FAIL  ") + c["name"] + ("" if c["ok"] or c["detail"] is None else f"  {json.dumps(c['detail'], ensure_ascii=False)}"))
    return 0 if checks and all(c["ok"] for c in checks) else 1


if __name__ == "__main__":
    sys.exit(main())

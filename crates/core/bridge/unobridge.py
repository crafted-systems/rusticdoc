#!/usr/bin/env python3
"""rusticdoc's LibreOffice bridge: one warm soffice, driven over UNO.

Started once per worker as
    python3 unobridge.py --socket-path NAME --profile DIR --soffice BIN
it launches soffice with a private profile, connects over a named pipe,
prints {"ready": true, "soffice_pid": N}, then serves one JSON request per
stdin line
    {"id": 1, "input": "/w/in.docx", "output": "/w/out.pdf", "filter": "pdf",
     "filter_options": {"SelectPdfVersion": 2}, "password": null, "landscape": false}
answering one JSON line each on stdout
    {"id": 1, "ok": true, "pages": 3, "ms": 412}  or  {"id": 1, "ok": false, "error": "..."}
`filter` is an export filter name, or a bare target extension (pdf, docx, ...)
resolved against the application that opened the document. When soffice dies
the bridge answers "libreoffice gone" and exits non-zero so a fresh worker is
started. Standard library plus `uno` (python3-uno) only.
"""
import argparse
import json
import os
import signal
import subprocess
import sys
import time

import uno  # noqa: E402  (python3-uno)
from com.sun.star.beans import PropertyValue  # noqa: E402
from com.sun.star.connection import NoConnectException  # noqa: E402
from com.sun.star.lang import DisposedException  # noqa: E402

EXIT_GONE = 3

# Export filter per application and target extension.
FILTERS = {
    "writer": {
        "pdf": "writer_pdf_Export", "docx": "MS Word 2007 XML", "doc": "MS Word 97",
        "odt": "writer8", "rtf": "Rich Text Format", "html": "HTML (StarWriter)",
        "txt": "Text",
    },
    "calc": {
        "pdf": "calc_pdf_Export", "xlsx": "Calc MS Excel 2007 XML", "xls": "MS Excel 97",
        "ods": "calc8", "csv": "Text - txt - csv (StarCalc)", "html": "HTML (StarCalc)",
    },
    "impress": {
        "pdf": "impress_pdf_Export", "pptx": "Impress MS PowerPoint 2007 XML",
        "ppt": "MS PowerPoint 97", "odp": "impress8", "html": "impress_html_Export",
    },
    "draw": {"pdf": "draw_pdf_Export", "odg": "draw8", "svg": "draw_svg_Export"},
    "web": {"pdf": "writer_web_pdf_Export", "html": "HTML", "txt": "Text (StarWriter/Web)"},
}
SERVICES = [
    ("com.sun.star.text.WebDocument", "web"),
    ("com.sun.star.text.TextDocument", "writer"),
    ("com.sun.star.sheet.SpreadsheetDocument", "calc"),
    ("com.sun.star.presentation.PresentationDocument", "impress"),
    ("com.sun.star.drawing.DrawingDocument", "draw"),
]


def props(mapping):
    return uno.Any("[]com.sun.star.beans.PropertyValue",
                   tuple(PropertyValue(Name=k, Value=v) for k, v in mapping.items()))


def set_pdeathsig():
    """Linux: have the kernel kill soffice when this process dies."""
    try:
        import ctypes
        ctypes.CDLL("libc.so.6", use_errno=True).prctl(1, signal.SIGKILL)
    except Exception:  # not Linux; the Rust side kills by pid instead
        pass


class Office:
    def __init__(self, soffice, profile, pipe):
        os.makedirs(profile, exist_ok=True)
        self.args = [
            soffice, "-env:UserInstallation=" + uno.systemPathToFileUrl(profile),
            "--headless", "--invisible", "--nocrashreport", "--nodefault", "--nologo",
            "--nofirststartwizard", "--norestore",
            "--accept=pipe,name=%s;urp;StarOffice.ComponentContext" % pipe]
        self.env = dict(os.environ, HOME=profile)
        self.launch()
        local = uno.getComponentContext()
        resolver = local.ServiceManager.createInstanceWithContext(
            "com.sun.star.bridge.UnoUrlResolver", local)
        deadline = time.monotonic() + 120
        restarted = False
        while True:
            try:
                self.ctx = resolver.resolve(
                    "uno:pipe,name=%s;urp;StarOffice.ComponentContext" % pipe)
                break
            except NoConnectException:
                code = self.proc.poll()
                if code == 81 and not restarted:  # fresh profile: start once more
                    restarted = True
                    self.launch()
                elif code is not None:
                    raise RuntimeError("libreoffice exited with %d during start" % code)
                if time.monotonic() > deadline:
                    self.proc.kill()
                    self.proc.wait()
                    raise RuntimeError("libreoffice did not accept a connection in time")
                time.sleep(0.05)
        self.desktop = self.ctx.ServiceManager.createInstanceWithContext(
            "com.sun.star.frame.Desktop", self.ctx)

    def launch(self):
        # Its own session, so a signal to the bridge's group misses it and
        # the Rust side kills it by pid; umask 077 keeps the UNO pipe private.
        self.proc = subprocess.Popen(
            self.args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            preexec_fn=set_pdeathsig, env=self.env, start_new_session=True)

    def alive(self):
        return self.proc.poll() is None

    def shutdown(self):
        try:
            self.desktop.terminate()
            self.proc.wait(timeout=5)
        except Exception:
            pass
        if self.alive():
            self.proc.kill()
            self.proc.wait()

    def convert(self, req):
        load = {"Hidden": True, "ReadOnly": True, "UpdateDocMode": 0,
                "MacroExecutionMode": 0}
        if req.get("password"):
            load["Password"] = req["password"]
        if req.get("landscape"):
            load["ReadOnly"] = False  # page styles are edited before export
        url = uno.systemPathToFileUrl(os.path.abspath(req["input"]))
        doc = self.desktop.loadComponentFromURL(url, "_blank", 0, props(load))
        if doc is None:
            raise RuntimeError("could not open the document (wrong password, damaged or unsupported)")
        try:
            app = next((a for s, a in SERVICES if doc.supportsService(s)), "writer")
            target = req["filter"]
            if " " not in target and "_" not in target:
                name = FILTERS.get(app, {}).get(target.lower())
                if name is None:
                    raise RuntimeError("a %s document cannot be exported to %s" % (app, target))
                target = name
            if req.get("landscape"):
                set_landscape(doc, app)
            store = {"FilterName": target, "Overwrite": True}
            options = req.get("filter_options") or {}
            if options:
                store["FilterData"] = props(options)
            doc.storeToURL(uno.systemPathToFileUrl(os.path.abspath(req["output"])),
                           props(store))
            return page_count(doc, app)
        finally:
            try:
                doc.close(True)
            except Exception:
                try:
                    doc.dispose()
                except Exception:
                    pass


def set_landscape(doc, app):
    """Best effort: turn every page style (or slide) to landscape."""
    try:
        if app in ("impress", "draw"):
            pages = doc.DrawPages
            for i in range(pages.Count):
                page = pages.getByIndex(i)
                w, h = page.Width, page.Height
                if w < h:
                    page.Width, page.Height = h, w
            return
        styles = doc.StyleFamilies.getByName("PageStyles")
        for name in styles.ElementNames:
            style = styles.getByName(name)
            if style.Width < style.Height:
                style.Width, style.Height = style.Height, style.Width
            style.IsLandscape = True
    except Exception:
        pass


def page_count(doc, app):
    try:
        if app in ("impress", "draw"):
            return doc.DrawPages.Count
        if app == "calc":
            return None
        return doc.CurrentController.PageCount
    except Exception:
        return None


def reply(obj):
    print(json.dumps(obj), flush=True)


def main():
    ap = argparse.ArgumentParser()
    for name in ("--socket-path", "--profile", "--soffice"):
        ap.add_argument(name, required=True)
    args = ap.parse_args()
    os.umask(0o077)
    try:
        office = Office(args.soffice, args.profile, args.socket_path)
    except Exception as e:
        reply({"ready": False, "error": str(e)})
        return 2
    reply({"ready": True, "soffice_pid": office.proc.pid})
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    try:
        while True:
            line = sys.stdin.readline()
            if not line:
                return 0
            line = line.strip()
            if not line:
                continue
            req = json.loads(line)
            started = time.monotonic()
            try:
                if not office.alive():
                    raise DisposedException("libreoffice gone", None)
                pages = office.convert(req)
                reply({"id": req.get("id"), "ok": True, "pages": pages,
                       "ms": int((time.monotonic() - started) * 1000)})
            except DisposedException:
                reply({"id": req.get("id"), "ok": False, "error": "libreoffice gone"})
                return EXIT_GONE
            except Exception as e:  # UNO exceptions carry their text in Message
                text = getattr(e, "Message", None) or str(e) or type(e).__name__
                if not office.alive():
                    reply({"id": req.get("id"), "ok": False, "error": "libreoffice gone"})
                    return EXIT_GONE
                reply({"id": req.get("id"), "ok": False, "error": text.strip()})
    finally:
        office.shutdown()


if __name__ == "__main__":
    sys.exit(main())

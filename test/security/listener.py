import http.server
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        open('hits.log','a').write(self.path+'\n'); self.send_response(200); self.end_headers(); self.wfile.write(b'x')
    do_HEAD=do_GET
    def log_message(self,*a): pass
http.server.HTTPServer(('0.0.0.0',3199),H).serve_forever()

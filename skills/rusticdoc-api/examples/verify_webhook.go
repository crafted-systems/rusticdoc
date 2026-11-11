// A rusticdoc webhook receiver that checks each delivery's signature.
//
//	RUSTICDOC_SIGNING_KEY=<hex> PORT=8000 go run verify_webhook.go
//
// Standard library only. RUSTICDOC_SIGNING_KEY is the hex that `rusticdoc
// webhook-secret` prints for the calling key, run where RUSTICDOC_WEBHOOK_SECRET
// is set. A verified result is saved in ./received.
package main

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"io"
	"log"
	"math"
	"mime"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const maxAgeSeconds = 300

var (
	key  []byte
	mu   sync.Mutex
	seen = map[string]bool{} // request ids already handled; deliveries can repeat
)

func verified(h http.Header, body []byte) bool {
	timestamp := h.Get("X-Rusticdoc-Timestamp")
	event := h.Get("X-Rusticdoc-Event")
	requestID := h.Get("X-Request-Id")
	signature := h.Get("X-Rusticdoc-Signature")
	ts, err := strconv.ParseInt(timestamp, 10, 64)
	if err != nil || event == "" || requestID == "" || !strings.HasPrefix(signature, "v1=") {
		return false
	}
	if math.Abs(float64(time.Now().Unix()-ts)) > maxAgeSeconds {
		return false
	}
	// Over the raw body bytes: a parsed or re-encoded body won't verify.
	mac := hmac.New(sha256.New, key)
	mac.Write([]byte(timestamp + "." + event + "." + requestID + "."))
	mac.Write(body)
	expected := hex.EncodeToString(mac.Sum(nil))
	return hmac.Equal([]byte(signature[3:]), []byte(expected))
}

func receive(w http.ResponseWriter, r *http.Request) {
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, 256<<20))
	if err != nil {
		http.Error(w, "unreadable body", http.StatusBadRequest)
		return
	}
	ok := verified(r.Header, body)
	event := r.Header.Get("X-Rusticdoc-Event")
	requestID := r.Header.Get("X-Request-Id")
	verdict := "REJECTED"
	if ok {
		verdict = "verified"
	}
	log.Println(verdict, event, requestID, len(body))
	if ok {
		mu.Lock()
		first := !seen[requestID]
		seen[requestID] = true
		mu.Unlock()
		if first && event == "render.completed" {
			name := requestID
			if _, params, err := mime.ParseMediaType(r.Header.Get("Content-Disposition")); err == nil && params["filename"] != "" {
				name = filepath.Base(params["filename"])
			}
			if err := os.MkdirAll("received", 0o755); err == nil {
				_ = os.WriteFile(filepath.Join("received", name), body, 0o644)
			}
		} else if first {
			log.Println("failure:", string(body))
		}
		// Any 2xx stops the retries; anything else asks for another attempt.
		w.WriteHeader(http.StatusNoContent)
		return
	}
	w.WriteHeader(http.StatusUnauthorized)
}

func main() {
	var err error
	key, err = hex.DecodeString(os.Getenv("RUSTICDOC_SIGNING_KEY"))
	if err != nil || len(key) == 0 {
		log.Fatal("set RUSTICDOC_SIGNING_KEY to the hex that rusticdoc webhook-secret printed")
	}
	port := os.Getenv("PORT")
	if port == "" {
		port = "8000"
	}
	log.Fatal(http.ListenAndServe(":"+port, http.HandlerFunc(receive)))
}

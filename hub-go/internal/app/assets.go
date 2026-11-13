package app

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"log"
	"mime"
	"net/http"
	"path"
	"strconv"
	"strings"
	"time"
	"unicode"

	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/store"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

// Assets (ADR 0011 B, person to agent): the chat composer uploads an image, the hub checks it, stores it in the
// database (content-addressed by SHA-256, so every pod can serve it) and sends the machine fetch_asset; the
// machine downloads GET /api/assets/{sha256} with its own token, writes the file and answers with its path.

const (
	maxImageBytes    = 10 << 20
	assetRetention   = 24 * time.Hour
	assetSweepEvery  = 10 * time.Minute
	assetFetchWithin = 60 * time.Second
)

// sniffImage is the image type the bytes are, from their magic numbers only (never the name or the claimed
// type); "" for anything that is not a png, jpeg, gif, webp, heic or avif.
func sniffImage(b []byte) string {
	has := func(sig string) bool { return bytes.HasPrefix(b, []byte(sig)) }
	ftyp := func(brands ...string) bool {
		if len(b) < 12 || string(b[4:8]) != "ftyp" {
			return false
		}
		for _, br := range brands {
			if string(b[8:12]) == br {
				return true
			}
		}
		return false
	}
	switch {
	case has("\x89PNG\r\n\x1a\n"):
		return "image/png"
	case has("\xff\xd8\xff"):
		return "image/jpeg"
	case has("GIF87a"), has("GIF89a"):
		return "image/gif"
	case len(b) >= 12 && has("RIFF") && string(b[8:12]) == "WEBP":
		return "image/webp"
	case ftyp("heic", "heix", "hevc", "mif1", "heim", "msf1"):
		return "image/heic"
	case ftyp("avif", "avis"):
		return "image/avif"
	}
	return ""
}

func imageExt(mime string) string {
	switch mime {
	case "image/png":
		return "png"
	case "image/jpeg":
		return "jpg"
	case "image/gif":
		return "gif"
	case "image/webp":
		return "webp"
	case "image/heic":
		return "heic"
	case "image/avif":
		return "avif"
	}
	return "bin"
}

// cleanAssetName is a file name fit for a header and a list: no path, no control characters, at most 120 chars.
func cleanAssetName(name string) string {
	n := path.Base(strings.ReplaceAll(name, "\\", "/"))
	if n == "." || n == "/" {
		n = ""
	}
	n = strings.TrimSpace(strings.Map(func(r rune) rune {
		if unicode.IsControl(r) || r == '"' || r == '/' {
			return -1
		}
		return r
	}, n))
	if r := []rune(n); len(r) > 120 {
		n = string(r[len(r)-120:])
	}
	if n == "" {
		return "image"
	}
	return n
}

func validSHA(s string) bool {
	if len(s) != 64 {
		return false
	}
	for _, c := range s {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}

// postUpload is the chat composer's image: multipart field "file" (at most 10 MB, an image by its bytes). The
// machine fetches it and answers with the absolute path it wrote; the reply is {name,path,sha256,size,mime}.
// By default the machine only stores the file (the person sends it with the next message, as the Blazor hub
// did); ?prompt=1 (and an optional form field "caption") makes the machine prompt the pane itself.
func (a *App) postUpload(w http.ResponseWriter, r *http.Request) {
	id, ok := a.machineOr404(w, r)
	if !ok {
		return
	}
	pane := r.PathValue("pane")
	if info, linked := a.Runtime.Info(id); !linked || !info.Online {
		jsonErr(w, http.StatusServiceUnavailable, "machine is offline")
		return
	} else if info.Protocol < 3 {
		jsonErr(w, http.StatusConflict, "machine "+id+" links with protocol "+strconv.Itoa(info.Protocol)+"; update klark there to receive images")
		return
	}
	r.Body = http.MaxBytesReader(w, r.Body, maxImageBytes+64<<10)
	f, hdr, err := r.FormFile("file")
	if err != nil {
		var tooBig *http.MaxBytesError
		if errors.As(err, &tooBig) {
			jsonErr(w, http.StatusRequestEntityTooLarge, "image is larger than 10 MB")
			return
		}
		jsonErr(w, http.StatusBadRequest, "send the image as multipart field \"file\"")
		return
	}
	defer f.Close()
	if r.MultipartForm != nil {
		defer func() { _ = r.MultipartForm.RemoveAll() }()
	}
	data, err := io.ReadAll(io.LimitReader(f, maxImageBytes+1))
	if err != nil {
		jsonErr(w, http.StatusBadRequest, "could not read the image")
		return
	}
	if len(data) > maxImageBytes {
		jsonErr(w, http.StatusRequestEntityTooLarge, "image is larger than 10 MB")
		return
	}
	if len(data) == 0 {
		jsonErr(w, http.StatusBadRequest, "the image is empty")
		return
	}
	mimeType := sniffImage(data)
	if mimeType == "" {
		jsonErr(w, http.StatusUnsupportedMediaType, "not an image; send png, jpeg, gif, webp, heic or avif")
		return
	}
	sum := sha256.Sum256(data)
	sha := hex.EncodeToString(sum[:])
	name := cleanAssetName(hdr.Filename)
	if !strings.Contains(name, ".") {
		name += "." + imageExt(mimeType)
	}
	me, _ := auth.FromContext(r.Context())
	if _, err := a.Store.PutAssetBlob(sha, mimeType, data); err != nil {
		jsonErr(w, 500, "could not store the image")
		return
	}
	if err := a.Store.AddAsset(store.Asset{SHA256: sha, Size: int64(len(data)), Mime: mimeType, Name: name, Time: time.Now().UnixMilli(),
		Machine: id, Pane: pane, Direction: "to_agent", By: "user:" + me.Name}); err != nil {
		jsonErr(w, 500, "could not record the image")
		return
	}
	prompt := r.URL.Query().Get("prompt") == "1" || r.URL.Query().Get("prompt") == "true"
	ctx, cancel := context.WithTimeout(r.Context(), assetFetchWithin)
	defer cancel()
	raw, err := a.Runtime.Request(ctx, id, map[string]any{"type": "fetch_asset", "sha256": sha, "name": name, "mime": mimeType,
		"size": len(data), "pane_id": pane, "caption": r.FormValue("caption"), "prompt": prompt}, assetFetchWithin)
	if err != nil {
		failure(w, err)
		return
	}
	var res struct {
		Path string `json:"path"`
	}
	_ = json.Unmarshal(raw, &res)
	log.Printf("asset %s (%d bytes) delivered to %s/%s by %s", sha[:12], len(data), id, pane, me.Name)
	jsonOut(w, http.StatusOK, map[string]any{"name": name, "path": res.Path, "sha256": sha, "size": len(data), "mime": mimeType})
}

// getAsset serves an asset's bytes. With a machine token (Authorization: Bearer) only an asset addressed to that
// machine; otherwise a signed-in person (cookie). Records past the retention are gone.
func (a *App) getAsset(w http.ResponseWriter, r *http.Request) {
	sha := strings.ToLower(r.PathValue("sha"))
	if !validSHA(sha) {
		jsonErr(w, http.StatusNotFound, "no such asset")
		return
	}
	var visible func(store.Asset) bool
	if h := r.Header.Get("Authorization"); len(h) > 7 && strings.EqualFold(h[:7], "Bearer ") {
		m := tokens.Match(a.Tokens.Machines(), strings.TrimSpace(h[7:]))
		if m == nil {
			jsonErr(w, http.StatusUnauthorized, "unknown machine token")
			return
		}
		me, refused := tokens.ResolveMachine(m, r.Header.Get("X-Klark-Machine"))
		if me == "" {
			jsonErr(w, http.StatusBadRequest, refused)
			return
		}
		visible = func(as store.Asset) bool { return as.Direction == "to_agent" && strings.EqualFold(as.Machine, me) }
	} else {
		u, ok := a.Auth.Current(r)
		if !ok {
			jsonErr(w, http.StatusUnauthorized, "sign in required")
			return
		}
		visible = func(as store.Asset) bool { return u.Sees(a.Store.MachineTenant(as.Machine)) }
	}
	rows, err := a.Store.Assets(sha, time.Now().Add(-assetRetention).UnixMilli())
	if err != nil {
		jsonErr(w, 500, "could not read assets")
		return
	}
	var found *store.Asset
	for i := range rows {
		if visible(rows[i]) {
			found = &rows[i]
			break
		}
	}
	if found == nil {
		jsonErr(w, http.StatusForbidden, "no such asset for you")
		return
	}
	data, err := a.Store.AssetBlob(sha)
	if err != nil {
		jsonErr(w, http.StatusNotFound, "no such asset")
		return
	}
	h := w.Header()
	h.Set("Content-Type", found.Mime)
	h.Set("Content-Length", strconv.Itoa(len(data)))
	h.Set("X-Content-Type-Options", "nosniff")
	h.Set("Cache-Control", "private, max-age=3600")
	h.Set("Content-Security-Policy", "default-src 'none'; sandbox")
	h.Set("Content-Disposition", mime.FormatMediaType("inline", map[string]string{"filename": found.Name}))
	_, _ = w.Write(data)
}

// sweepAssets drops records past the retention and the bytes nothing names any more, every 10 minutes.
func (a *App) sweepAssets(ctx context.Context) {
	t := time.NewTicker(assetSweepEvery)
	defer t.Stop()
	for {
		if recs, blobs, err := a.Store.SweepAssets(time.Now().Add(-assetRetention).UnixMilli()); err != nil {
			log.Printf("asset sweep: %v", err)
		} else if recs > 0 || blobs > 0 {
			log.Printf("asset sweep: %d records and %d images past 24 h removed", recs, blobs)
		}
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
	}
}

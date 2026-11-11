package ui

import (
	"net/http/httptest"
	"strings"
	"testing"
	"testing/fstest"
)

func TestIndexBase(t *testing.T) {
	cases := map[string]string{
		`<head><base href="__HUB_BASE_HREF__"></head>`: `<base href="/klark/">`,
		`<head><base href="/"></head>`:                 `<base href="/klark/">`,
		`<head></head>`:                                `<head><base href="/klark/">`,
	}
	for in, want := range cases {
		if got := string(IndexHTML([]byte(in), "/klark")); !strings.Contains(got, want) {
			t.Errorf("%q -> %q", in, got)
		}
	}
	if got := string(IndexHTML([]byte(`<base href="__HUB_BASE_HREF__">`), "")); got != `<base href="/">` {
		t.Error(got)
	}
}

func TestHandlerSPAAndCache(t *testing.T) {
	fsys := fstest.MapFS{
		"index.html":    {Data: []byte(`<head><base href="__HUB_BASE_HREF__"></head>app`)},
		"assets/a-1.js": {Data: []byte("x")},
		"favicon.svg":   {Data: []byte("<svg/>")},
	}
	h := HandlerFS(fsys, "/k")
	get := func(p string) *httptest.ResponseRecorder {
		w := httptest.NewRecorder()
		h.ServeHTTP(w, httptest.NewRequest("GET", p, nil))
		return w
	}
	if w := get("/m/mac/p/1"); !strings.Contains(w.Body.String(), `<base href="/k/">`) || w.Header().Get("Cache-Control") != "no-cache" {
		t.Fatalf("spa fallback: %v %s", w.Header(), w.Body)
	}
	if w := get("/assets/a-1.js"); !strings.Contains(w.Header().Get("Cache-Control"), "immutable") {
		t.Fatal("hashed asset not immutable")
	}
	if w := get("/favicon.svg"); w.Header().Get("Cache-Control") != "no-cache" {
		t.Fatal("favicon must revalidate")
	}
	if w := get("/assets/gone-2.js"); w.Code != 404 {
		t.Fatalf("stale asset = %d", w.Code)
	}
}

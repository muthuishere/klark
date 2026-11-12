package stats

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

// Fake test credentials (not real secrets).
const (
	testKey    = "testkey123"
	testSecret = "testsecret123456"
)

// fakeS3 is an in-memory S3 that verifies the AWS Signature V4 of every request the way S3 does.
type fakeS3 struct {
	mu   sync.Mutex
	objs map[string][]byte
	bad  []string
}

func (f *fakeS3) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	auth := r.Header.Get("Authorization")
	// Credential=key/date/region/s3/aws4_request, SignedHeaders=..., Signature=...
	parts := map[string]string{}
	for _, p := range strings.Split(strings.TrimPrefix(auth, "AWS4-HMAC-SHA256 "), ", ") {
		k, v, _ := strings.Cut(p, "=")
		parts[k] = v
	}
	cred := strings.Split(parts["Credential"], "/")
	sum := sha256.Sum256(body)
	ok := len(cred) == 5 && cred[0] == testKey && r.Header.Get("x-amz-content-sha256") == hex.EncodeToString(sum[:])
	if ok {
		var ch strings.Builder
		for _, h := range strings.Split(parts["SignedHeaders"], ";") {
			v := r.Header.Get(h)
			if h == "host" {
				v = r.Host
			}
			ch.WriteString(h + ":" + v + "\n")
		}
		canon := strings.Join([]string{r.Method, r.URL.EscapedPath(), r.URL.RawQuery, ch.String(), parts["SignedHeaders"], r.Header.Get("x-amz-content-sha256")}, "\n")
		cs := sha256.Sum256([]byte(canon))
		scope := strings.Join(cred[1:], "/")
		toSign := "AWS4-HMAC-SHA256\n" + r.Header.Get("x-amz-date") + "\n" + scope + "\n" + hex.EncodeToString(cs[:])
		k := []byte("AWS4" + testSecret)
		for _, s := range []string{cred[1], cred[2], cred[3], cred[4]} {
			m := hmac.New(sha256.New, k)
			m.Write([]byte(s))
			k = m.Sum(nil)
		}
		m := hmac.New(sha256.New, k)
		m.Write([]byte(toSign))
		ok = hex.EncodeToString(m.Sum(nil)) == parts["Signature"]
	}
	if !ok {
		f.mu.Lock()
		f.bad = append(f.bad, r.Method+" "+r.URL.Path)
		f.mu.Unlock()
		http.Error(w, "<Error><Code>SignatureDoesNotMatch</Code></Error>", 403)
		return
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	switch r.Method {
	case "PUT":
		f.objs[r.URL.Path] = body
	case "DELETE":
		delete(f.objs, r.URL.Path)
		w.WriteHeader(204)
	case "GET":
		if r.URL.Query().Get("list-type") == "2" {
			bucket := strings.TrimSuffix(r.URL.Path, "/")
			pre := bucket + "/" + r.URL.Query().Get("prefix")
			var x strings.Builder
			x.WriteString("<ListBucketResult><IsTruncated>false</IsTruncated>")
			for k := range f.objs {
				if strings.HasPrefix(k, pre) {
					x.WriteString("<Contents><Key>" + strings.TrimPrefix(k, bucket+"/") + "</Key></Contents>")
				}
			}
			x.WriteString("</ListBucketResult>")
			_, _ = w.Write([]byte(x.String()))
			return
		}
		b, found := f.objs[r.URL.Path]
		if !found {
			http.Error(w, "NoSuchKey", 404)
			return
		}
		_, _ = w.Write(b)
	}
}

func archiveRoundTrip(t *testing.T, sink *S3Sink, wantPathPrefix func(string) string) {
	t.Helper()
	st := newStore(t)
	now := time.Date(2026, 3, 4, 15, 20, 0, 0, time.UTC)
	_ = st.InsertStat(store.StatRow{MachineID: "a", MachineName: "Alpha", TS: now.UnixMilli(), RSSBytes: 42, AgentsJSON: "[]"})
	ar := &Archiver{Store: st, Now: func() time.Time { return now }, Config: func() config.Storage { return config.Storage{Kind: "s3"} },
		MakeSink: func(config.Storage) (Sink, error) { return sink, nil }}
	ar.Cycle(context.Background())
	if s := ar.Status(); s.LastError != "" || s.FilesWritten != 1 {
		t.Fatalf("%+v", s)
	}
	key := Key(ar.HubID(), now)
	b, err := sink.Get(context.Background(), key)
	if err != nil {
		t.Fatal(err)
	}
	lines := gunzipLines(t, b)
	if len(lines) != 1 || lines[0]["klark_rss_bytes"].(float64) != 42 || lines[0]["machine"] != "Alpha" {
		t.Fatalf("%v", lines)
	}
	// idempotent: the same hour again overwrites the same object
	ar2 := &Archiver{Store: st, Now: ar.Now, Config: ar.Config, MakeSink: ar.MakeSink}
	ar2.ResetCursor()
	ar2.Cycle(context.Background())
	b2, _ := sink.Get(context.Background(), key)
	if !bytes.Equal(b, b2) {
		t.Fatal("overwrite changed the bytes")
	}
	keys, err := sink.List(context.Background(), "analytics/")
	if err != nil || len(keys) != 1 || keys[0] != key {
		t.Fatalf("list %v %v", keys, err)
	}
	if ok, msg := ar.Test(context.Background(), config.Storage{Kind: "s3"}); !ok {
		t.Fatal(msg)
	}
	if _, err := sink.Get(context.Background(), "analytics/.klark-test-"+ar.HubID()); err == nil {
		t.Fatal("marker object must be deleted")
	}
	// wrong secret: refused, and the secret is not in the error
	bad, _ := NewS3Sink(config.S3Config{Bucket: sink.cfg.Bucket, Endpoint: sink.cfg.Endpoint, PathStyle: true, AccessKeyID: testKey, SecretAccessKey: "wrongwrongwrong1"}, "")
	err = bad.Put(context.Background(), "x", []byte("y"))
	if err == nil || strings.Contains(err.Error(), "wrongwrongwrong1") {
		t.Fatalf("bad secret: %v", err)
	}
}

func TestS3SinkAgainstFakeS3VerifyingSigV4(t *testing.T) {
	f := &fakeS3{objs: map[string][]byte{}}
	ts := httptest.NewServer(f)
	defer ts.Close()
	sink, err := NewS3Sink(config.S3Config{Bucket: "bkt", Endpoint: ts.URL, PathStyle: true, AccessKeyID: testKey, SecretAccessKey: testSecret}, "/my prefix/x")
	if err != nil {
		t.Fatal(err)
	}
	archiveRoundTrip(t, sink, nil)
	if len(f.bad) != 1 { // only the deliberate wrong-secret request
		t.Fatalf("signature failures: %v", f.bad)
	}
	for k := range f.objs {
		if !strings.HasPrefix(k, "/bkt/my%20prefix/x/analytics/2026/03/04/15-") && !strings.HasPrefix(k, "/bkt/my prefix/x/analytics/2026/03/04/15-") {
			t.Fatalf("object path %q", k)
		}
	}
	if _, err := NewS3Sink(config.S3Config{Bucket: "b", AccessKeyID: "k"}, ""); err == nil {
		t.Fatal("missing secret accepted")
	}
	if _, err := NewS3Sink(config.S3Config{Bucket: "b", AccessKeyID: "k", SecretAccessKey: "s", Endpoint: "nope"}, ""); err == nil {
		t.Fatal("bad endpoint accepted")
	}
}

// TestS3SinkAgainstSeaweedFS runs the same round trip against a real S3 server (SeaweedFS in docker) when docker and
// the pinned image are present; otherwise it is skipped.
func TestS3SinkAgainstSeaweedFS(t *testing.T) {
	endpoint := startSeaweedFS(t, "klark-s3-stats-test")
	var sink *S3Sink
	deadline := time.Now().Add(60 * time.Second)
	for {
		sink, _ = NewS3Sink(config.S3Config{Bucket: "klark", Endpoint: endpoint, PathStyle: true, AccessKeyID: testKey, SecretAccessKey: testSecret, Region: "us-east-1"}, "pfx")
		if err := sink.Put(context.Background(), "ready", []byte("1")); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the S3 endpoint did not accept writes")
		}
		time.Sleep(time.Second)
	}
	archiveRoundTrip(t, sink, nil)
}

// startSeaweedFS runs a pinned SeaweedFS (Apache-2.0) with one S3 access key and the bucket "klark", and returns its
// S3 endpoint. It skips the test when docker or the image is missing (KLARK_TEST_S3_IMAGE overrides the image).
func startSeaweedFS(t *testing.T, name string) string {
	t.Helper()
	if testing.Short() {
		t.Skip("short")
	}
	image := "chrislusf/seaweedfs:4.00"
	if v := os.Getenv("KLARK_TEST_S3_IMAGE"); v != "" {
		image = v
	}
	if exec.Command("docker", "image", "inspect", image).Run() != nil {
		t.Skip("docker or image " + image + " not available")
	}
	_ = exec.Command("docker", "rm", "-f", name).Run()
	script := `cat > /tmp/s3.json <<EOF
{"identities":[{"name":"t","credentials":[{"accessKey":"` + testKey + `","secretKey":"` + testSecret + `"}],"actions":["Admin","Read","Write","List","Tagging"]}]}
EOF
exec weed server -dir=/data -s3 -s3.port=8333 -s3.config=/tmp/s3.json -master.volumeSizeLimitMB=128 -volume.max=4`
	if out, err := exec.Command("docker", "run", "-d", "--name", name, "-p", "127.0.0.1:0:8333", "--entrypoint", "/bin/sh", image, "-c", script).CombinedOutput(); err != nil {
		t.Skip("cannot start seaweedfs: " + string(out))
	}
	t.Cleanup(func() { _ = exec.Command("docker", "rm", "-f", "-v", name).Run() })
	deadline := time.Now().Add(90 * time.Second)
	for { // create the bucket once the master and filer are up
		out, _ := exec.Command("docker", "exec", name, "sh", "-c", `echo "s3.bucket.create -name klark" | weed shell -master=localhost:9333 2>&1`).CombinedOutput()
		if strings.Contains(string(out), "created bucket klark") || strings.Contains(string(out), "exist") {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("seaweedfs did not become ready: %s", out)
		}
		time.Sleep(2 * time.Second)
	}
	pb, err := exec.Command("docker", "port", name, "8333/tcp").Output()
	if err != nil {
		t.Fatal(err)
	}
	return "http://" + strings.TrimSpace(strings.Split(string(pb), "\n")[0])
}

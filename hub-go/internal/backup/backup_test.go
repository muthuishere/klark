package backup

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/stats"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

const (
	testKey    = "testkey123"
	testSecret = "testsecret123456"
)

func openDB(t *testing.T, dir string) *store.Store {
	t.Helper()
	s, err := store.Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func manager(s *store.Store, dir string, sink stats.Sink, pod string, now *time.Time) *Manager {
	return &Manager{Store: s, Dir: dir, Pod: pod, Keep: 2, Sink: func() (stats.Sink, error) { return sink, nil }, Now: func() time.Time { return *now }}
}

func scenario(t *testing.T, sink stats.Sink) {
	t.Helper()
	ctx := context.Background()
	dir := t.TempDir()
	s := openDB(t, dir)
	now := time.Date(2026, 5, 1, 10, 0, 0, 0, time.UTC)
	m := manager(s, dir, sink, "pod-a", &now)

	_ = s.SetSetting("k", "v1")
	m.Cycle(ctx)
	if st := m.Status(); st.LastError != "" || st.Uploads != 1 || st.LastBackupMs != now.UnixMilli() {
		t.Fatalf("%+v", st)
	}
	snaps, _ := Snapshots(ctx, sink)
	if len(snaps) != 1 {
		t.Fatalf("%v", snaps)
	}
	// nothing changed: skipped, no second file
	now = now.Add(time.Hour)
	m.Cycle(ctx)
	if snaps, _ = Snapshots(ctx, sink); len(snaps) != 1 || m.Status().Uploads != 1 {
		t.Fatalf("unchanged data must not be uploaded again: %v", snaps)
	}
	// changes: new snapshots, only the last Keep=2 stay
	for i, v := range []string{"v2", "v3", "v4"} {
		now = now.Add(time.Hour)
		_ = s.SetSetting("k", v)
		m.Cycle(ctx)
		_ = i
	}
	if snaps, _ = Snapshots(ctx, sink); len(snaps) != 2 || m.Status().Uploads != 4 {
		t.Fatalf("keep 2: %v", snaps)
	}
	// owner marker: another hub is refused while the marker is fresh, accepted once it expired
	if err := Claim(ctx, sink, "pod-b", now); !errors.As(err, new(ErrOwned)) || !strings.Contains(err.Error(), "pod-a") {
		t.Fatalf("second hub must be refused: %v", err)
	}

	// a new pod with an empty disk restores the newest snapshot
	_ = s.SetSetting("k", "v5")
	now = now.Add(time.Hour)
	m.Final(ctx) // clean shutdown: uploads v5 and releases the marker
	if _, err := sink.Get(ctx, ownerKey); !errors.Is(err, stats.ErrNotFound) {
		t.Fatalf("marker must be released: %v", err)
	}
	dir2 := t.TempDir()
	restored, err := Start(ctx, sink, "pod-b", filepath.Join(dir2, "hub.db"), 0)
	if err != nil || !restored {
		t.Fatalf("restored=%v %v", restored, err)
	}
	s2 := openDB(t, dir2)
	if v, _ := s2.Setting("k"); v != "v5" {
		t.Fatalf("restored value %q", v)
	}
	_ = s2.Close()
	// pod-b now owns the location; a local db newer than the newest snapshot is kept, an older one is replaced
	if _, err := Start(ctx, sink, "pod-c", filepath.Join(dir2, "x.db"), 0); !errors.As(err, new(ErrOwned)) {
		t.Fatalf("pod-c must be refused while pod-b owns it: %v", err)
	}
	if restored, err = Start(ctx, sink, "pod-b", filepath.Join(dir2, "hub.db"), 0); err != nil || restored {
		t.Fatalf("a newer local db must be kept: %v %v", restored, err)
	}
	old := time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC)
	_ = os.Chtimes(filepath.Join(dir2, "hub.db"), old, old)
	_ = os.Remove(filepath.Join(dir2, "hub.db-wal"))
	if restored, err = Start(ctx, sink, "pod-b", filepath.Join(dir2, "hub.db"), 0); err != nil || !restored {
		t.Fatalf("an older local db must be replaced: %v %v", restored, err)
	}
	if m, _ := filepath.Glob(filepath.Join(dir2, "hub.db.before-restore-*")); len(m) != 1 {
		t.Fatal("the replaced db must be kept aside")
	}
}

func TestBackupToFolder(t *testing.T) {
	scenario(t, &stats.FolderSink{Root: filepath.Join(t.TempDir(), "parent")})
}

func TestBackupToSeaweedFS(t *testing.T) {
	endpoint := startSeaweedFS(t, "klark-s3-backup-test")
	var sink *stats.S3Sink
	deadline := time.Now().Add(60 * time.Second)
	for {
		sink, _ = stats.NewS3Sink(config.S3Config{Bucket: "klark", Endpoint: endpoint, PathStyle: true, AccessKeyID: testKey, SecretAccessKey: testSecret}, "klarkhub/prod")
		if err := sink.Put(context.Background(), "ready", []byte("1")); err == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the S3 endpoint did not accept writes")
		}
		time.Sleep(time.Second)
	}
	scenario(t, sink)
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

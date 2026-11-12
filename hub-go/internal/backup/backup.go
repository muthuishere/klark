// Package backup keeps a SQLite hub.db restorable: a consistent snapshot (VACUUM INTO, gzip) is uploaded to the
// storage target every HUB_SQLITE_BACKUP_EVERY (skipped when nothing changed) and on clean shutdown; on start a
// missing or older hub.db is restored from the newest snapshot. One owner marker per location keeps two hubs from
// backing up to (or restoring from) the same place. The data-loss window after a crash is up to the interval.
package backup

import (
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/stats"
	"github.com/muthuishere/klark/hub-go/internal/store"
)

const (
	snapPrefix = "sqlite/hub-"
	ownerKey   = "sqlite/owner.json"
	stampFmt   = "20060102T150405Z"
	// OwnerTTL is how long an owner marker without heartbeat blocks another hub (heartbeats come every 30 s).
	OwnerTTL = 90 * time.Second
)

type marker struct {
	Pod string `json:"pod"`
	Ms  int64  `json:"ms"`
}

// ErrOwned means another live hub owns the backup location.
type ErrOwned struct{ Pod string }

func (e ErrOwned) Error() string {
	return "the backup location is owned by hub " + e.Pod + " (heartbeat within the last " + OwnerTTL.String() +
		"); only one hub may back up to a location. Stop it, wait for the marker to expire, or use another HUB_BACKUP_PATH"
}

// Claim takes (or renews) the owner marker. It fails with ErrOwned when another hub's marker is fresh.
func Claim(ctx context.Context, sink stats.Sink, pod string, now time.Time) error {
	b, err := sink.Get(ctx, ownerKey)
	if err != nil && !errors.Is(err, stats.ErrNotFound) {
		return err
	}
	if err == nil {
		var m marker
		if json.Unmarshal(b, &m) == nil && m.Pod != pod && now.Sub(time.UnixMilli(m.Ms)) < OwnerTTL {
			return ErrOwned{m.Pod}
		}
	}
	nb, _ := json.Marshal(marker{Pod: pod, Ms: now.UnixMilli()})
	return sink.Put(ctx, ownerKey, nb)
}

// release removes the marker when it is ours.
func release(ctx context.Context, sink stats.Sink, pod string) {
	if b, err := sink.Get(ctx, ownerKey); err == nil {
		var m marker
		if json.Unmarshal(b, &m) == nil && m.Pod == pod {
			_ = sink.Delete(ctx, ownerKey)
		}
	}
}

func snapKey(t time.Time) string { return snapPrefix + t.UTC().Format(stampFmt) + ".db.gz" }

func snapTime(key string) (time.Time, bool) {
	k := strings.TrimSuffix(strings.TrimPrefix(key, snapPrefix), ".db.gz")
	t, err := time.Parse(stampFmt, k)
	return t, err == nil
}

// Snapshots lists the snapshot keys, oldest first.
func Snapshots(ctx context.Context, sink stats.Sink) ([]string, error) {
	keys, err := sink.List(ctx, snapPrefix)
	if err != nil {
		return nil, err
	}
	var out []string
	for _, k := range keys {
		if _, ok := snapTime(k); ok {
			out = append(out, k)
		}
	}
	sort.Strings(out)
	return out, nil
}

// Start runs before the database is opened: it claims the location (waiting up to wait for another hub's marker to
// expire) and restores dbPath from the newest snapshot when hub.db is missing or older than it. It reports whether
// it restored.
func Start(ctx context.Context, sink stats.Sink, pod, dbPath string, wait time.Duration) (restored bool, err error) {
	deadline := time.Now().Add(wait)
	for {
		err = Claim(ctx, sink, pod, time.Now())
		var owned ErrOwned
		if err == nil || !errors.As(err, &owned) || time.Now().After(deadline) {
			break
		}
		log.Printf("backup: %v; waiting for it to expire", err)
		select {
		case <-ctx.Done():
			return false, ctx.Err()
		case <-time.After(5 * time.Second):
		}
	}
	if err != nil {
		return false, err
	}
	snaps, err := Snapshots(ctx, sink)
	if err != nil || len(snaps) == 0 {
		return false, err
	}
	newest := snaps[len(snaps)-1]
	st, _ := snapTime(newest)
	local, have := latestMod(dbPath)
	if have && !local.Before(st) {
		return false, nil
	}
	if err := restore(ctx, sink, newest, dbPath, have); err != nil {
		return false, fmt.Errorf("restoring %s: %w", newest, err)
	}
	log.Printf("backup: restored %s from %s", filepath.Base(dbPath), newest)
	return true, nil
}

// latestMod is the newest modification time of hub.db and its WAL.
func latestMod(dbPath string) (time.Time, bool) {
	var t time.Time
	ok := false
	for _, p := range []string{dbPath, dbPath + "-wal"} {
		if fi, err := os.Stat(p); err == nil {
			ok = ok || p == dbPath
			if fi.ModTime().After(t) {
				t = fi.ModTime()
			}
		}
	}
	return t, ok
}

func restore(ctx context.Context, sink stats.Sink, key, dbPath string, haveLocal bool) error {
	b, err := sink.Get(ctx, key)
	if err != nil {
		return err
	}
	zr, err := gzip.NewReader(bytes.NewReader(b))
	if err != nil {
		return err
	}
	raw, err := io.ReadAll(zr)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(dbPath), 0o750); err != nil {
		return err
	}
	tmp := dbPath + ".restore"
	if err := os.WriteFile(tmp, raw, 0o640); err != nil {
		return err
	}
	if s, err := store.OpenPath(tmp); err != nil { // the snapshot must open and migrate
		_ = os.Remove(tmp)
		return err
	} else {
		_ = s.Close()
	}
	for _, ext := range []string{"-wal", "-shm"} {
		_ = os.Remove(tmp + ext)
	}
	if haveLocal {
		_ = os.Rename(dbPath, dbPath+".before-restore-"+time.Now().UTC().Format(stampFmt))
	}
	for _, ext := range []string{"-wal", "-shm"} {
		_ = os.Remove(dbPath + ext)
	}
	return os.Rename(tmp, dbPath)
}

// Manager runs the periodic snapshots of a SQLite store.
type Manager struct {
	Store *store.Store
	// Sink is the destination in force right now (it can change when an admin saves other settings); nil = off.
	Sink  func() (stats.Sink, error)
	Pod   string
	Dir   string // scratch directory for the snapshot file (the data dir)
	Every time.Duration
	Keep  int
	Now   func() time.Time

	mu       sync.Mutex
	lastSHA  string
	lastOK   int64
	lastErr  string
	owned    bool
	uploaded int64
}

// Status is what the admin page shows.
type Status struct {
	LastBackupMs int64
	LastError    string
	Uploads      int64
	Owner        string
}

func (m *Manager) now() time.Time {
	if m.Now != nil {
		return m.Now()
	}
	return time.Now()
}

func (m *Manager) Status() Status {
	m.mu.Lock()
	defer m.mu.Unlock()
	return Status{m.lastOK, m.lastErr, m.uploaded, m.Pod}
}

func (m *Manager) fail(err error) {
	if err != nil {
		log.Printf("sqlite backup: %v", err)
	}
	m.mu.Lock()
	m.lastErr = ""
	if err != nil {
		m.lastErr = err.Error()
	}
	m.mu.Unlock()
}

// Run snapshots every Every and renews the owner marker every 30 s until ctx ends.
func (m *Manager) Run(ctx context.Context) {
	every := m.Every
	if every <= 0 {
		every = 6 * time.Hour
	}
	snap := time.NewTicker(every)
	beat := time.NewTicker(30 * time.Second)
	defer snap.Stop()
	defer beat.Stop()
	m.Cycle(ctx) // the first snapshot goes out at start (skipped when identical to the last one)
	for {
		select {
		case <-ctx.Done():
			return
		case <-beat.C:
			if sink, err := m.Sink(); err == nil && sink != nil {
				if err := Claim(ctx, sink, m.Pod, m.now()); err != nil {
					m.fail(err)
				}
			}
		case <-snap.C:
			m.Cycle(ctx)
		}
	}
}

// Cycle takes a snapshot and uploads it unless nothing changed since the last upload.
func (m *Manager) Cycle(ctx context.Context) {
	sink, err := m.Sink()
	if err != nil {
		m.fail(err)
		return
	}
	if sink == nil {
		return
	}
	m.fail(m.snapshotAndUpload(ctx, sink))
}

func (m *Manager) snapshotAndUpload(ctx context.Context, sink stats.Sink) error {
	if err := Claim(ctx, sink, m.Pod, m.now()); err != nil {
		return err
	}
	dir := m.Dir
	if dir == "" {
		dir = os.TempDir()
	}
	tmp := filepath.Join(dir, fmt.Sprintf(".backup-%d.db", m.now().UnixNano()))
	if err := m.Store.Snapshot(tmp); err != nil {
		return err
	}
	defer os.Remove(tmp)
	raw, err := os.ReadFile(tmp)
	if err != nil {
		return err
	}
	sum := sha256.Sum256(raw)
	h := hex.EncodeToString(sum[:])
	m.mu.Lock()
	same := h == m.lastSHA
	m.mu.Unlock()
	if same {
		return nil
	}
	var buf bytes.Buffer
	zw, _ := gzip.NewWriterLevel(&buf, gzip.BestCompression)
	_, _ = zw.Write(raw)
	_ = zw.Close()
	now := m.now()
	if err := sink.Put(ctx, snapKey(now), buf.Bytes()); err != nil {
		return err
	}
	m.mu.Lock()
	m.lastSHA, m.lastOK = h, now.UnixMilli()
	m.uploaded++
	m.mu.Unlock()
	return m.prune(ctx, sink)
}

func (m *Manager) prune(ctx context.Context, sink stats.Sink) error {
	keep := m.Keep
	if keep < 1 {
		keep = 7
	}
	snaps, err := Snapshots(ctx, sink)
	if err != nil {
		return err
	}
	for len(snaps) > keep {
		if err := sink.Delete(ctx, snaps[0]); err != nil {
			return err
		}
		snaps = snaps[1:]
	}
	return nil
}

// Final is the clean-shutdown upload (when something changed) and releases the owner marker. Call it before the
// store is closed.
func (m *Manager) Final(ctx context.Context) {
	sink, err := m.Sink()
	if err != nil || sink == nil {
		return
	}
	m.fail(m.snapshotAndUpload(ctx, sink))
	release(ctx, sink, m.Pod)
}

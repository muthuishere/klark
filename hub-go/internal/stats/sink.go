package stats

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"github.com/muthuishere/klark/hub-go/internal/config"
)

// Sink stores one archive object under a slash separated key.
type Sink interface {
	Put(ctx context.Context, key string, data []byte) error
	Delete(ctx context.Context, key string) error
	// Get returns ErrNotFound when the object does not exist.
	Get(ctx context.Context, key string) ([]byte, error)
	// List returns the keys that start with prefix, sorted.
	List(ctx context.Context, prefix string) ([]string, error)
}

// ErrNotFound is returned by Get for a missing object.
var ErrNotFound = errors.New("not found")

// NewSink builds the sink for the storage target; nil, nil for kind none.
func NewSink(a config.Storage) (Sink, error) {
	switch a.Kind {
	case "", "none":
		return nil, nil
	case "folder":
		if strings.TrimSpace(a.Path) == "" {
			return nil, errors.New("folder: the path is empty")
		}
		return &FolderSink{Root: a.Path}, nil
	case "s3":
		return NewS3Sink(a.S3, a.Path)
	}
	return nil, fmt.Errorf("unknown archive kind %q", a.Kind)
}

// FolderSink writes under Root and never outside it.
type FolderSink struct{ Root string }

func (f *FolderSink) path(key string) (string, error) {
	if key == "" || strings.Contains(key, "\x00") {
		return "", errors.New("bad key")
	}
	for _, seg := range strings.Split(key, "/") {
		if seg == ".." {
			return "", errors.New("key escapes the archive folder")
		}
	}
	root, err := filepath.Abs(f.Root)
	if err != nil {
		return "", err
	}
	p := filepath.Join(root, filepath.FromSlash(key))
	if rel, err := filepath.Rel(root, p); err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) || filepath.IsAbs(rel) {
		return "", errors.New("key escapes the archive folder")
	}
	return p, nil
}

func (f *FolderSink) Put(_ context.Context, key string, data []byte) error {
	p, err := f.path(key)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o750); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(filepath.Dir(p), ".tmp-*")
	if err != nil {
		return err
	}
	_, werr := tmp.Write(data)
	if cerr := tmp.Close(); werr == nil {
		werr = cerr
	}
	if werr != nil {
		_ = os.Remove(tmp.Name())
		return werr
	}
	if err := os.Chmod(tmp.Name(), 0o640); err != nil {
		_ = os.Remove(tmp.Name())
		return err
	}
	if err := os.Rename(tmp.Name(), p); err != nil {
		_ = os.Remove(tmp.Name())
		return err
	}
	return nil
}

func (f *FolderSink) Get(_ context.Context, key string) ([]byte, error) {
	p, err := f.path(key)
	if err != nil {
		return nil, err
	}
	b, err := os.ReadFile(p)
	if os.IsNotExist(err) {
		return nil, ErrNotFound
	}
	return b, err
}

func (f *FolderSink) List(_ context.Context, prefix string) ([]string, error) {
	root, err := filepath.Abs(f.Root)
	if err != nil {
		return nil, err
	}
	var out []string
	_ = filepath.WalkDir(root, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		rel, _ := filepath.Rel(root, p)
		rel = filepath.ToSlash(rel)
		if strings.HasPrefix(rel, prefix) && !strings.Contains(filepath.Base(rel), ".tmp-") {
			out = append(out, rel)
		}
		return nil
	})
	sort.Strings(out)
	return out, nil
}

func (f *FolderSink) Delete(_ context.Context, key string) error {
	p, err := f.path(key)
	if err != nil {
		return err
	}
	if err := os.Remove(p); err != nil && !os.IsNotExist(err) {
		return err
	}
	return nil
}

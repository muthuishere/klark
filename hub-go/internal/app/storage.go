package app

import (
	"encoding/json"
	"net/http"
	"os"
	"strings"

	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/stats"
)

const settingStorage = "storage.settings"

// storageConfig is the storage target in force and where it comes from: the admin-saved settings (kept in the
// database) replace the environment's as a whole.
func (a *App) storageConfig() (config.Storage, string) {
	if v, ok := a.Store.Setting(settingStorage); ok && v != "" {
		var c config.Storage
		if json.Unmarshal([]byte(v), &c) == nil {
			if c.Kind == "" {
				c.Kind = "none"
			}
			return c, "admin"
		}
	}
	c := a.Cfg.Storage
	if c.Kind == "" {
		c.Kind = "none"
	}
	return c, "env"
}

type storageS3View struct {
	Bucket             string `json:"bucket"`
	Region             string `json:"region"`
	Endpoint           string `json:"endpoint"`
	AccessKeyID        string `json:"accessKeyId"`
	SecretAccessKeySet bool   `json:"secretAccessKeySet"`
	PathStyle          bool   `json:"pathStyle"`
}

func (a *App) storageView() map[string]any {
	c, src := a.storageConfig()
	an := a.Archive.Status()
	sq := map[string]any{"enabled": a.Backup != nil && c.Kind != "none", "every": a.Cfg.SQLiteBackupEvery.String(), "keep": a.Cfg.SQLiteBackupKeep}
	var lastBackup int64
	lastErr := an.LastError
	if a.Backup != nil {
		st := a.Backup.Status()
		lastBackup, sq["owner"], sq["backups"] = st.LastBackupMs, st.Owner, st.Uploads
		if st.LastError != "" {
			lastErr = st.LastError
		}
	}
	return map[string]any{"kind": c.Kind, "source": src, "path": c.Path,
		"s3":                   storageS3View{c.S3.Bucket, c.S3.Region, c.S3.Endpoint, c.S3.AccessKeyID, c.S3.SecretAccessKey != "", c.S3.PathStyle},
		"sqlite":               sq,
		"lastSqliteBackupMs":   lastBackup,
		"lastAnalyticsWriteMs": an.LastWriteMs, "lastError": lastErr, "filesWritten": an.FilesWritten}
}

func (a *App) adminStorage(w http.ResponseWriter, _ *http.Request) { jsonOut(w, 200, a.storageView()) }

// storageInput is the PUT/test body: the GET shape plus s3.secretAccessKey and clearSecretAccessKey.
type storageInput struct {
	Kind                 string          `json:"kind"`
	Path                 string          `json:"path"`
	S3                   config.S3Config `json:"s3"`
	ClearSecretAccessKey bool            `json:"clearSecretAccessKey"`
}

type errString string

func (e errString) Error() string { return string(e) }

// resolveStorage validates the input. A secret that is not sent is kept from the current setting when the access
// key id is unchanged.
func (a *App) resolveStorage(in storageInput) (config.Storage, error) {
	cur, _ := a.storageConfig()
	c := config.Storage{Kind: strings.ToLower(strings.TrimSpace(in.Kind)), Path: strings.TrimSpace(in.Path), S3: in.S3}
	c.S3.Bucket, c.S3.Region = strings.TrimSpace(c.S3.Bucket), strings.TrimSpace(c.S3.Region)
	c.S3.Endpoint, c.S3.AccessKeyID = strings.TrimSpace(c.S3.Endpoint), strings.TrimSpace(c.S3.AccessKeyID)
	switch {
	case in.ClearSecretAccessKey && in.S3.SecretAccessKey == "":
		c.S3.SecretAccessKey = ""
	case in.S3.SecretAccessKey == "" && cur.S3.AccessKeyID == c.S3.AccessKeyID:
		c.S3.SecretAccessKey = cur.S3.SecretAccessKey
	}
	switch c.Kind {
	case "":
		c.Kind = "none"
	case "none", "folder", "s3":
	default:
		return c, errString("kind must be none, folder or s3")
	}
	if c.Kind != "none" {
		if _, err := stats.NewSink(c); err != nil {
			return c, err
		}
	}
	return c, nil
}

func (a *App) adminPutStorage(w http.ResponseWriter, r *http.Request) {
	var in storageInput
	if !decode(w, r, &in) {
		return
	}
	c, err := a.resolveStorage(in)
	if err != nil {
		jsonErr(w, 400, err.Error())
		return
	}
	if c.Kind == "folder" {
		if err := os.MkdirAll(c.Path, 0o750); err != nil {
			jsonErr(w, 400, "folder: "+err.Error())
			return
		}
	}
	b, _ := json.Marshal(c)
	if err := a.Store.SetSetting(settingStorage, string(b)); err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	a.Archive.ResetCursor()
	jsonOut(w, 200, a.storageView())
}

func (a *App) adminResetStorage(w http.ResponseWriter, _ *http.Request) {
	if err := a.Store.SetSetting(settingStorage, ""); err != nil {
		jsonErr(w, 500, err.Error())
		return
	}
	a.Archive.ResetCursor()
	jsonOut(w, 200, a.storageView())
}

// adminTestStorage writes and deletes a marker object in the posted settings (or, with no body, the current ones).
func (a *App) adminTestStorage(w http.ResponseWriter, r *http.Request) {
	var in storageInput
	cfg, _ := a.storageConfig()
	if r.ContentLength != 0 {
		if !decode(w, r, &in) {
			return
		}
		if in.Kind != "" {
			c, err := a.resolveStorage(in)
			if err != nil {
				jsonOut(w, 200, map[string]any{"ok": false, "message": err.Error()})
				return
			}
			cfg = c
		}
	}
	if cfg.Kind == "folder" {
		if err := os.MkdirAll(cfg.Path, 0o750); err != nil {
			jsonOut(w, 200, map[string]any{"ok": false, "message": "folder: " + err.Error()})
			return
		}
	}
	ok, msg := a.Archive.Test(r.Context(), cfg)
	jsonOut(w, 200, map[string]any{"ok": ok, "message": msg})
}

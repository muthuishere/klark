package store

import (
	"errors"
	"testing"
)

// backends runs f against a fresh SQLite store directory.
func backends(t *testing.T, f func(t *testing.T, open func() *Store)) {
	dir := t.TempDir()
	f(t, func() *Store {
		s, err := Open(dir)
		if err != nil {
			t.Fatal(err)
		}
		return s
	})
}

func TestMigrationsAreIdempotent(t *testing.T) {
	backends(t, func(t *testing.T, open func() *Store) {
		for i := 0; i < 3; i++ {
			_ = open().Close()
		}
	})
}

func TestUsersMachinesMessagesSettings(t *testing.T) {
	backends(t, func(t *testing.T, open func() *Store) { usersMachinesMessagesSettings(t, open()) })
}

func usersMachinesMessagesSettings(t *testing.T, s *Store) {
	defer s.Close()

	if ok, _ := s.InsertUser(User{Name: "Alice", Hash: "h", Source: "local"}); !ok {
		t.Fatal("insert")
	}
	if ok, _ := s.InsertUser(User{Name: "alice", Hash: "h2"}); ok {
		t.Fatal("names are case-insensitive unique")
	}
	u, err := s.User("ALICE")
	if err != nil || u.DisplayName != "Alice" || u.Admin {
		t.Fatalf("%+v %v", u, err)
	}
	_ = s.UpsertUser(User{Name: "alice", Hash: "h3", Admin: true, Source: "env"})
	u, _ = s.User("alice")
	if !u.Admin || u.Hash != "h3" {
		t.Fatalf("%+v", u)
	}
	if err := s.DeleteUser("nobody"); !errors.Is(err, ErrNotFound) {
		t.Fatal(err)
	}

	if ok, _ := s.AddMachine(Machine{Name: "mac", SHA256: "ABCD"}); !ok {
		t.Fatal("machine")
	}
	ms, _ := s.Machines()
	if len(ms) != 1 || ms[0].SHA256 != "abcd" {
		t.Fatalf("%+v", ms)
	}
	s.TouchMachine("mac", "0.9", 5)
	ms, _ = s.Machines()
	if ms[0].Version != "0.9" || ms[0].LastSeen == 0 {
		t.Fatalf("%+v", ms)
	}
	if err := s.DeleteMachine("mac"); err != nil {
		t.Fatal(err)
	}

	for _, id := range []string{"a", "b", "c"} {
		_ = s.RecordMessage(Message{ID: id, Time: 100, Sender: "user:x", From: "user:x", To: "m/s/a", Text: id, OK: true})
	}
	got, _ := s.RecentMessages(2, "")
	if len(got) != 2 || got[0].ID != "c" {
		t.Fatalf("%+v", got)
	}

	if _, ok := s.Setting("k"); ok {
		t.Fatal("unset")
	}
	_ = s.SetSetting("k", "v1")
	_ = s.SetSetting("k", "v2")
	if v, _ := s.Setting("k"); v != "v2" {
		t.Fatal(v)
	}
}

func TestStats(t *testing.T) {
	backends(t, func(t *testing.T, open func() *Store) {
		s := open()
		defer s.Close()
		for i := int64(1); i <= 3; i++ {
			if err := s.InsertStat(StatRow{MachineID: "m", MachineName: "M", TS: i * 1000, RSSBytes: i * 10, CPUPercent: 1.5, AgentsJSON: "[]"}); err != nil {
				t.Fatal(err)
			}
		}
		rows, err := s.StatsBetween(1000, 3000)
		if err != nil || len(rows) != 2 || rows[0].RSSBytes != 10 || rows[0].CPUPercent != 1.5 {
			t.Fatalf("%+v %v", rows, err)
		}
		if r, ok, _ := s.LatestStat("m"); !ok || r.TS != 3000 {
			t.Fatal("latest")
		}
		if n, _ := s.PurgeStats(2500); n != 2 {
			t.Fatalf("purged %d", n)
		}
	})
}

package store

import (
	"errors"
	"testing"
)

func TestMigrationsAreIdempotent(t *testing.T) {
	dir := t.TempDir()
	for i := 0; i < 3; i++ {
		s, err := Open(dir)
		if err != nil {
			t.Fatal(err)
		}
		_ = s.Close()
	}
}

func TestUsersMachinesMessagesSettings(t *testing.T) {
	s, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
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
	got, _ := s.RecentMessages(2)
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

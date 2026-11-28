package app

import (
	"testing"
	"time"
)

func TestSubmitDelayMatchesMachineRule(t *testing.T) {
	cases := map[int]time.Duration{
		10:         300 * time.Millisecond,
		1600:       400 * time.Millisecond,
		10_000_000: 1500 * time.Millisecond,
	}
	for n, want := range cases {
		if got := SubmitDelay(n); got != want {
			t.Errorf("SubmitDelay(%d) = %v, want %v", n, got, want)
		}
	}
}

//go:build windows

package stats

import (
	"syscall"
	"time"
)

// procUsage on Windows reports CPU time only (GetProcessTimes); memory needs psapi, which is left out (0 = unknown).
func procUsage() (cpu time.Duration, rss, peak int64) {
	h, err := syscall.GetCurrentProcess()
	if err != nil {
		return 0, 0, 0
	}
	var c, e, k, u syscall.Filetime
	if syscall.GetProcessTimes(h, &c, &e, &k, &u) != nil {
		return 0, 0, 0
	}
	ft := func(f syscall.Filetime) int64 { return int64(f.HighDateTime)<<32 | int64(f.LowDateTime) } // 100 ns units
	return time.Duration((ft(k) + ft(u)) * 100), 0, 0
}

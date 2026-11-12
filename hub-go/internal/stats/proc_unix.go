//go:build unix

package stats

import (
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// procUsage is the process's CPU time (user+system), its peak RSS and, where cheap, its current RSS (0 = unknown).
func procUsage() (cpu time.Duration, rss, peak int64) {
	var ru syscall.Rusage
	if syscall.Getrusage(syscall.RUSAGE_SELF, &ru) == nil {
		cpu = time.Duration(ru.Utime.Nano() + ru.Stime.Nano())
		peak = maxrssBytes(int64(ru.Maxrss))
	}
	return cpu, currentRSS(), peak
}

// maxrssBytes: ru_maxrss is bytes on macOS and the BSD-derived iOS, kilobytes on Linux and the other BSDs.
func maxrssBytes(v int64) int64 {
	if runtime.GOOS == "darwin" || runtime.GOOS == "ios" {
		return v
	}
	return v * 1024
}

func currentRSS() int64 {
	if b, err := os.ReadFile("/proc/self/statm"); err == nil { // Linux: "size resident ..." in pages
		f := strings.Fields(string(b))
		if len(f) > 1 {
			if n, err := strconv.ParseInt(f[1], 10, 64); err == nil {
				return n * int64(os.Getpagesize())
			}
		}
	}
	if runtime.GOOS == "darwin" { // no /proc and no cgo: ps answers in KiB (only on a lazy sample, at most once per interval)
		out, err := exec.Command("ps", "-o", "rss=", "-p", strconv.Itoa(os.Getpid())).Output()
		if err == nil {
			if n, err := strconv.ParseInt(strings.TrimSpace(string(out)), 10, 64); err == nil {
				return n * 1024
			}
		}
	}
	return 0
}

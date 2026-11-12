//go:build !unix && !windows

package stats

import "time"

func procUsage() (cpu time.Duration, rss, peak int64) { return 0, 0, 0 }

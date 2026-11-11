package auth

import (
	"sync"
	"time"
)

// Limiter locks a key out after too many failures inside a window. Success clears the key.
type Limiter struct {
	Max    int
	Window time.Duration
	Now    func() time.Time

	mu sync.Mutex
	m  map[string][]time.Time
}

func NewLimiter(max int, window time.Duration) *Limiter {
	return &Limiter{Max: max, Window: window, Now: time.Now, m: map[string][]time.Time{}}
}

func (l *Limiter) prune(key string, now time.Time) []time.Time {
	var keep []time.Time
	for _, t := range l.m[key] {
		if now.Sub(t) < l.Window {
			keep = append(keep, t)
		}
	}
	if len(keep) == 0 {
		delete(l.m, key)
	} else {
		l.m[key] = keep
	}
	return keep
}

// Blocked reports whether key is locked out, and for how long.
func (l *Limiter) Blocked(key string) (bool, time.Duration) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.Now()
	fails := l.prune(key, now)
	if len(fails) < l.Max {
		return false, 0
	}
	return true, l.Window - now.Sub(fails[0])
}

func (l *Limiter) Fail(key string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := l.Now()
	l.prune(key, now)
	l.m[key] = append(l.m[key], now)
	if len(l.m) > 10000 { // bound memory under a spray of keys
		for k := range l.m {
			if len(l.prune(k, now)) == 0 {
				continue
			}
		}
	}
}

func (l *Limiter) Reset(key string) {
	l.mu.Lock()
	delete(l.m, key)
	l.mu.Unlock()
}

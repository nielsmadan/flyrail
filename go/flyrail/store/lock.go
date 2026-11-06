package store

import (
	"context"
	"sync"
	"time"
)

const lockPoll = 20 * time.Millisecond

type unlocker struct {
	once    sync.Once
	release func() error
	err     error
}

// NewUnlocker returns an Unlocker that runs release at most once and returns its first result on every call.
func NewUnlocker(release func() error) Unlocker {
	return &unlocker{release: release}
}

// Unlock runs the release function on the first call and returns its result on every call.
func (u *unlocker) Unlock() error {
	u.once.Do(func() {
		if u.release != nil {
			u.err = u.release()
		}
	})
	return u.err
}

func liveContext(ctx context.Context) context.Context {
	if ctx == nil {
		return context.Background()
	}
	return ctx
}

// Attempts once even when the timeout or context has already ended.
func acquire(ctx context.Context, name string, timeout time.Duration, attempt func() (acquired bool, wake <-chan struct{}, err error)) error {
	ctx = liveContext(ctx)
	started := time.Now()
	for {
		acquired, wake, err := attempt()
		if err != nil || acquired {
			return err
		}
		remaining := timeout - time.Since(started)
		if remaining <= 0 || ctx.Err() != nil {
			return NewBusyError(name, ctx.Err())
		}
		if wake == nil {
			remaining = min(remaining, lockPoll)
		}
		timer := time.NewTimer(remaining)
		select {
		case <-ctx.Done():
			timer.Stop()
			return NewBusyError(name, ctx.Err())
		case <-wake:
			timer.Stop()
		case <-timer.C:
		}
	}
}

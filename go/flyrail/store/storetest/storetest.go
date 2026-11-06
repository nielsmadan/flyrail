package storetest

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/nielsmadan/flyrail/go/flyrail/store"
)

// Run checks the state store contract on two handles from open, which must share the same data and hold nothing under conformance/.
func Run(t *testing.T, open func(t *testing.T) store.Store) {
	t.Helper()
	first, second := open(t), open(t)
	if err := check(t.Context(), first, second); err != nil {
		t.Fatal(err)
	}
}

func check(ctx context.Context, first, second store.Store) error {
	k := &kit{ctx: ctx}
	defer k.release()
	checkRecords(k, first, second)
	checkAbsentAndEmpty(k, first, second)
	checkCopies(k, first, second)
	checkConcurrentWrites(k, first, second)
	checkKeyValidation(k, first)
	checkLocks(k, first, second)
	checkLockErrors(k, first, second)
	checkLockContext(k, first, second)
	return k.err
}

type kit struct {
	ctx  context.Context
	err  error
	held []store.Unlocker
}

func (k *kit) fail(err error) {
	if k.err == nil && err != nil {
		k.err = err
	}
}

func (k *kit) expect(condition bool, message string) {
	if k.err == nil && !condition {
		k.err = errors.New(message)
	}
}

func (k *kit) read(s store.Store, key string) ([]byte, bool) {
	if k.err != nil {
		return nil, false
	}
	value, ok, err := s.Read(k.ctx, key)
	k.fail(err)
	return value, ok
}

func (k *kit) write(s store.Store, key string, value []byte, expected store.Expected) bool {
	if k.err != nil {
		return false
	}
	ok, err := s.Write(k.ctx, key, value, expected)
	k.fail(err)
	return ok
}

func (k *kit) delete(s store.Store, key string, expected []byte) bool {
	if k.err != nil {
		return false
	}
	ok, err := s.Delete(k.ctx, key, expected)
	k.fail(err)
	return ok
}

func (k *kit) keys(s store.Store, prefix string) []string {
	if k.err != nil {
		return nil
	}
	keys, err := s.Keys(k.ctx, prefix)
	k.fail(err)
	return keys
}

func (k *kit) lock(ctx context.Context, s store.Store, name string, timeout time.Duration) store.Unlocker {
	if k.err != nil {
		return nil
	}
	held, err := s.Lock(ctx, name, timeout)
	k.fail(err)
	if held != nil {
		k.held = append(k.held, held)
	}
	return held
}

func (k *kit) unlock(held store.Unlocker) {
	if k.err == nil && held != nil {
		k.fail(held.Unlock())
	}
}

func (k *kit) busy(ctx context.Context, s store.Store, name string, timeout time.Duration) (bool, error) {
	if k.err != nil {
		return false, nil
	}
	held, err := s.Lock(ctx, name, timeout)
	if err == nil {
		k.fail(held.Unlock())
		return false, nil
	}
	if !errors.Is(err, store.ErrBusy) {
		k.fail(err)
		return false, err
	}
	return true, err
}

func (k *kit) release() {
	for _, held := range slices.Backward(k.held) {
		_ = held.Unlock()
	}
}

func equal(value []byte, ok bool, want string) bool {
	return ok && bytes.Equal(value, []byte(want))
}

func checkRecords(k *kit, first, second store.Store) {
	_, ok := k.read(first, "conformance/absent")
	k.expect(!ok, "absent keys must read as None")
	k.expect(k.write(first, "conformance/a", []byte("one"), store.Absent()), "creating an absent key must succeed")
	value, ok := k.read(second, "conformance/a")
	k.expect(equal(value, ok, "one"), "a write must be visible through another handle")
	k.expect(!k.write(second, "conformance/a", []byte("two"), store.Absent()), "creating an existing key must fail")
	k.expect(!k.write(second, "conformance/a", []byte("two"), store.Equal([]byte("other"))), "a stale expected value must fail")
	value, ok = k.read(first, "conformance/a")
	k.expect(equal(value, ok, "one"), "a failed write must leave the value unchanged")
	k.expect(k.write(second, "conformance/a", []byte("two"), store.Equal([]byte("one"))), "a matching expected value must succeed")
	k.expect(k.write(first, "conformance/empty", []byte{}, store.Absent()), "creating an empty value must succeed")
	value, ok = k.read(second, "conformance/empty")
	k.expect(equal(value, ok, ""), "empty values must round-trip")
	binary := make([]byte, 256)
	for index := range binary {
		binary[index] = byte(index)
	}
	k.expect(k.write(first, "conformance/binary", binary, store.Absent()), "creating a binary value must succeed")
	value, ok = k.read(second, "conformance/binary")
	k.expect(ok && bytes.Equal(value, binary), "binary values must round-trip exactly")
	k.expect(k.write(first, "conformance/ab", []byte("x"), store.Absent()), "creating a sibling key must succeed")
	k.expect(slices.Equal(k.keys(second, "conformance/a"), []string{"conformance/a", "conformance/ab"}),
		"keys must list prefix matches in sorted order")
	k.expect(!k.delete(first, "conformance/a", []byte("one")), "a stale delete must fail")
	k.expect(k.delete(first, "conformance/a", []byte("two")), "a matching delete must succeed")
	_, ok = k.read(second, "conformance/a")
	k.expect(!ok, "a deleted key must read as None")
	k.expect(!k.delete(first, "conformance/a", []byte("two")), "deleting an absent key must fail")
	k.expect(slices.Equal(k.keys(second, "conformance/"), []string{"conformance/ab", "conformance/binary", "conformance/empty"}),
		"deleted keys must not be listed")
}

func checkConcurrentWrites(k *kit, first, second store.Store) {
	const writers = 16
	k.expect(k.write(first, "conformance/race", []byte("start"), store.Absent()), "creating the contended key must succeed")
	var (
		ready, done sync.WaitGroup
		start       = make(chan struct{})
		mu          sync.Mutex
		winners     []string
	)
	ready.Add(writers)
	done.Add(writers)
	for index := range writers {
		handle := first
		if index%2 == 1 {
			handle = second
		}
		id := fmt.Sprintf("writer-%d", index)
		go func() {
			defer done.Done()
			ready.Done()
			<-start
			won, err := handle.Write(k.ctx, "conformance/race", []byte(id), store.Equal([]byte("start")))
			mu.Lock()
			defer mu.Unlock()
			k.fail(err)
			if won {
				winners = append(winners, id)
			}
		}()
	}
	ready.Wait()
	close(start)
	done.Wait()
	k.expect(len(winners) == 1, fmt.Sprintf("concurrent writes with the same expected value must have exactly one winner, got %d", len(winners)))
	if len(winners) == 1 {
		value, ok := k.read(second, "conformance/race")
		k.expect(equal(value, ok, winners[0]), "the stored value must be the winner's")
	}
}

func checkAbsentAndEmpty(k *kit, first, second store.Store) {
	k.expect(!k.write(first, "conformance/empty", []byte("x"), store.Absent()), "creating a key that holds an empty value must fail")
	k.expect(!k.write(first, "conformance/missing", []byte("x"), store.Equal(nil)), "an empty expected value must not match an absent key")
	k.expect(!k.delete(first, "conformance/missing", []byte{}), "deleting an absent key with an empty expected value must fail")
	_, ok := k.read(second, "conformance/missing")
	k.expect(!ok, "a failed conditional write must not create the key")
	k.expect(k.write(second, "conformance/empty", []byte("full"), store.Equal(nil)), "an empty expected value must match an empty value")
	k.expect(k.delete(first, "conformance/empty", []byte("full")), "a matching delete of a replaced empty value must succeed")
}

func checkCopies(k *kit, first, second store.Store) {
	written := []byte("copy")
	k.expect(k.write(first, "conformance/copy", written, store.Absent()), "creating a key must succeed")
	written[0] = 'X'
	value, ok := k.read(second, "conformance/copy")
	k.expect(equal(value, ok, "copy"), "a store must copy written values")
	if len(value) > 0 {
		value[0] = 'Y'
	}
	value, ok = k.read(first, "conformance/copy")
	k.expect(equal(value, ok, "copy"), "a store must return copies of stored values")
	expected := []byte("copy")
	k.expect(k.delete(second, "conformance/copy", expected), "a matching delete must succeed")
	_, ok = k.read(first, "conformance/copy")
	k.expect(!ok, "a deleted key must read as None")
}

func checkKeyValidation(k *kit, first store.Store) {
	for _, key := range []string{"", "../escape", "Upper/case", "trailing/", "a//b", "-dash", strings.Repeat("a", 201)} {
		if k.err != nil {
			return
		}
		_, _, err := first.Read(k.ctx, key)
		switch {
		case err == nil:
			k.fail(fmt.Errorf("invalid key %q must fail with ErrInvalidKey", key))
		case !errors.Is(err, store.ErrInvalidKey):
			k.fail(err)
		}
	}
	if k.err != nil {
		return
	}
	_, writeErr := first.Write(k.ctx, "Upper", []byte("x"), store.Absent())
	_, deleteErr := first.Delete(k.ctx, "Upper", []byte("x"))
	_, lockErr := first.Lock(k.ctx, "Upper", 0)
	k.expect(errors.Is(writeErr, store.ErrInvalidKey), "writing an invalid key must fail with ErrInvalidKey")
	k.expect(errors.Is(deleteErr, store.ErrInvalidKey), "deleting an invalid key must fail with ErrInvalidKey")
	k.expect(errors.Is(lockErr, store.ErrInvalidKey), "locking an invalid name must fail with ErrInvalidKey")
}

func checkLocks(k *kit, first, second store.Store) {
	held := k.lock(k.ctx, first, "conformance/lock", 0)
	excluded, _ := k.busy(k.ctx, second, "conformance/lock", 0)
	k.expect(excluded, "a held lock must exclude another handle")
	started := time.Now()
	excluded, _ = k.busy(k.ctx, second, "conformance/lock", 200*time.Millisecond)
	k.expect(excluded, "a held lock must exclude a bounded wait")
	k.expect(time.Since(started) >= 150*time.Millisecond, "a bounded lock wait must wait before failing")
	excluded, _ = k.busy(k.ctx, second, "conformance/other", 0)
	k.expect(!excluded, "different lock names must not exclude")
	k.unlock(held)
	excluded, _ = k.busy(k.ctx, second, "conformance/lock", 0)
	k.expect(!excluded, "a released lock must be acquirable")
}

func checkLockErrors(k *kit, first, second store.Store) {
	held := k.lock(k.ctx, first, "conformance/lock", 0)
	_, err := k.busy(k.ctx, second, "conformance/lock", 0)
	busy, ok := errors.AsType[*store.BusyError](err)
	k.expect(ok && busy.Name == "conformance/lock" && err.Error() == "state store lock is busy: conformance/lock",
		"a busy lock must fail with *BusyError naming the lock")
	k.unlock(held)
	k.expect(held == nil || held.Unlock() == nil, "a repeated Unlock must succeed")
	again := k.lock(k.ctx, second, "conformance/lock", 0)
	k.expect(held == nil || held.Unlock() == nil, "a repeated Unlock must succeed")
	excluded, _ := k.busy(k.ctx, first, "conformance/lock", 0)
	k.expect(excluded, "a repeated Unlock must not release another holder's lock")
	k.unlock(again)
}

func checkLockContext(k *kit, first, second store.Store) {
	canceled, cancel := context.WithCancel(k.ctx)
	cancel()
	free := k.lock(canceled, first, "conformance/free", 0)
	k.expect(free != nil, "the first lock attempt must not depend on the context")
	k.unlock(free)
	held := k.lock(k.ctx, first, "conformance/lock", 0)
	excluded, err := k.busy(canceled, second, "conformance/lock", 0)
	k.expect(excluded && errors.Is(err, context.Canceled), "a busy lock with an ended context must wrap the context error")
	started := time.Now()
	excluded, err = k.busy(canceled, second, "conformance/lock", time.Minute)
	k.expect(excluded && errors.Is(err, context.Canceled), "a canceled context must end a lock wait")
	k.expect(time.Since(started) < 30*time.Second, "a canceled context must end a lock wait before its timeout")
	deadline, stop := context.WithTimeout(k.ctx, 100*time.Millisecond)
	defer stop()
	started = time.Now()
	excluded, err = k.busy(deadline, second, "conformance/lock", time.Minute)
	elapsed := time.Since(started)
	k.expect(excluded && errors.Is(err, context.DeadlineExceeded), "a context deadline must end a lock wait")
	k.expect(elapsed >= 50*time.Millisecond, "a context deadline must not end a lock wait early")
	k.expect(elapsed < 30*time.Second, "a context deadline must end a lock wait before its timeout")
	k.unlock(held)
}

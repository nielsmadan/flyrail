package store

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"io"
	"os"
	"slices"
	"strings"
	"syscall"
	"time"

	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
	"github.com/nielsmadan/flyrail/go/internal/unicodex"
)

const (
	mutationTimeout = 10 * time.Second
	// maxRecordBytes is twice a journal's four hex-encoded revisions at the 256 MiB resource limit.
	maxRecordBytes    int64 = 4 << 30
	temporaryAttempts       = 16
)

// DirectoryStore keeps records, locks and staging space under one private directory, compatible with the other Flyrail implementations.
type DirectoryStore struct {
	root      string
	records   string
	trusted   []string
	maxRecord int64
}

// NewDirectoryStore returns a store rooted at path made absolute; nothing is created until the first write or lock.
func NewDirectoryStore(_ context.Context, path string) (*DirectoryStore, error) {
	if err := platformSupport(path); err != nil {
		return nil, err
	}
	root, err := absolute(path)
	if err != nil {
		return nil, err
	}
	records := pathx.Join(root, "records")
	return &DirectoryStore{
		root:      root,
		records:   records,
		trusted:   []string{root, records, pathx.Join(root, "locks"), pathx.Join(root, "staging")},
		maxRecord: maxRecordBytes,
	}, nil
}

// Location returns the absolute store directory.
func (s *DirectoryStore) Location() (string, bool) {
	if s == nil || s.root == "" {
		return "", false
	}
	return s.root, true
}

// StagingDirectory returns the store's staging directory, <location>/staging.
func (s *DirectoryStore) StagingDirectory() (string, bool) {
	if s == nil || s.root == "" {
		return "", false
	}
	return pathx.Join(s.root, "staging"), true
}

// Read returns the value stored at key; ok is false when the key is absent.
func (s *DirectoryStore) Read(_ context.Context, key string) ([]byte, bool, error) {
	path, err := s.record(key)
	if err != nil {
		return nil, false, err
	}
	if err := requirePrivate(s.trusted); err != nil {
		return nil, false, err
	}
	return readRecord(path, s.maxRecord)
}

// Write replaces the value at key when the key currently holds expected, and reports whether it did.
func (s *DirectoryStore) Write(ctx context.Context, key string, value []byte, expected Expected) (bool, error) {
	path, err := s.record(key)
	if err != nil {
		return false, err
	}
	if int64(len(value)) > s.maxRecord {
		return false, ioFailure(syscall.EFBIG, path)
	}
	mutation, err := s.mutation(ctx)
	if err != nil {
		return false, err
	}
	defer func() { _ = mutation.Unlock() }()
	current, ok, err := s.Read(ctx, key)
	if err != nil || !matches(current, ok, expected) {
		return false, err
	}
	if err := s.replace(path, value); err != nil {
		return false, err
	}
	return true, nil
}

// Delete removes key when it currently holds exactly expected, and reports whether it did.
func (s *DirectoryStore) Delete(ctx context.Context, key string, expected []byte) (bool, error) {
	path, err := s.record(key)
	if err != nil {
		return false, err
	}
	mutation, err := s.mutation(ctx)
	if err != nil {
		return false, err
	}
	defer func() { _ = mutation.Unlock() }()
	current, ok, err := s.Read(ctx, key)
	if err != nil || !ok || !bytes.Equal(current, expected) {
		return false, err
	}
	if err := removeFile(path); err != nil {
		return false, ioFailure(err, path)
	}
	if err := syncDirectory(s.records); err != nil {
		return false, err
	}
	return true, nil
}

// Keys returns the stored keys starting with prefix in ascending byte order.
func (s *DirectoryStore) Keys(_ context.Context, prefix string) ([]string, error) {
	if err := s.check(); err != nil {
		return nil, err
	}
	if err := requirePrivate(s.trusted); err != nil {
		return nil, err
	}
	names, err := directoryNames(s.records)
	if errors.Is(err, syscall.ENOENT) {
		return []string{}, nil
	}
	if err != nil {
		return nil, ioFailure(err, s.records)
	}
	keys := []string{}
	for _, name := range names {
		key := strings.ReplaceAll(unicodex.Lossy(name), "~", "/")
		if !strings.HasPrefix(name, ".") && strings.HasPrefix(key, prefix) {
			keys = append(keys, key)
		}
	}
	slices.Sort(keys)
	return keys, nil
}

// Lock acquires the named lock, waiting up to timeout and while ctx lasts; a zero timeout makes exactly one attempt.
func (s *DirectoryStore) Lock(ctx context.Context, name string, timeout time.Duration) (Unlocker, error) {
	if err := ValidateLockName(name); err != nil {
		return nil, err
	}
	if err := s.check(); err != nil {
		return nil, err
	}
	return holdFileLock(ctx, pathx.Join(s.root, "locks"), s.trusted, name, timeout)
}

func (s *DirectoryStore) check() error {
	if s == nil || s.root == "" {
		return &IOError{Message: "directory state store must come from NewDirectoryStore"}
	}
	return nil
}

func (s *DirectoryStore) record(key string) (string, error) {
	if err := ValidateKey(key); err != nil {
		return "", err
	}
	if err := s.check(); err != nil {
		return "", err
	}
	return pathx.Join(s.records, strings.ReplaceAll(key, "/", "~")), nil
}

func (s *DirectoryStore) mutation(ctx context.Context) (Unlocker, error) {
	return holdFileLock(ctx, s.root, s.trusted, "mutation", mutationTimeout)
}

func (s *DirectoryStore) replace(path string, value []byte) error {
	if err := privateDirectory(s.records); err != nil {
		return err
	}
	name := unicodex.Lossy(path[strings.LastIndexByte(path, '/')+1:])
	file, temporary, err := createTemporary(s.records, name)
	if err != nil {
		return err
	}
	if err := writeAndSync(file, value); err != nil {
		_ = os.Remove(temporary)
		return ioFailure(err, temporary)
	}
	if err := renameReplacing(temporary, path); err != nil {
		_ = os.Remove(temporary)
		return ioFailure(err, path)
	}
	return syncDirectory(s.records)
}

func createTemporary(directory, name string) (*os.File, string, error) {
	for attempt := 1; ; attempt++ {
		temporary := pathx.Join(directory, "."+name+"."+temporarySuffix()+".tmp")
		file, err := os.OpenFile(temporary, os.O_WRONLY|os.O_CREATE|os.O_EXCL|createFlags, 0o600)
		if err == nil {
			return file, temporary, nil
		}
		if !errors.Is(err, syscall.EEXIST) || attempt == temporaryAttempts {
			return nil, "", ioFailure(err, temporary)
		}
	}
}

func temporarySuffix() string {
	var random [8]byte
	_, _ = rand.Read(random[:])
	return hex.EncodeToString(random[:])
}

func writeAndSync(file *os.File, value []byte) error {
	_, err := file.Write(value)
	if err == nil {
		err = file.Sync()
	}
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	return err
}

func readRecord(path string, limit int64) ([]byte, bool, error) {
	file, err := os.Open(path)
	if errors.Is(err, syscall.ENOENT) {
		return nil, false, nil
	}
	if err != nil {
		return nil, false, ioFailure(err, path)
	}
	defer func() { _ = file.Close() }()
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil {
		return nil, false, ioFailure(err, path)
	}
	if int64(len(data)) > limit {
		return nil, false, ioFailure(syscall.EFBIG, path)
	}
	return nonNil(data), true, nil
}

func directoryNames(path string) ([]string, error) {
	directory, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer func() { _ = directory.Close() }()
	return directory.Readdirnames(-1)
}

func holdFileLock(ctx context.Context, directory string, trusted []string, name string, timeout time.Duration) (Unlocker, error) {
	if err := requirePrivate(trusted); err != nil {
		return nil, err
	}
	if err := privateDirectory(directory); err != nil {
		return nil, err
	}
	path := pathx.Join(directory, strings.ReplaceAll(name, "/", "~"))
	lock, err := sysx.OpenLockFile(path)
	if err != nil {
		return nil, ioFailure(err, path)
	}
	err = acquire(ctx, name, timeout, func() (bool, <-chan struct{}, error) {
		acquired, err := lock.TryLock()
		if err != nil {
			return false, nil, ioFailure(err, path)
		}
		return acquired, nil, nil
	})
	if err != nil {
		_ = lock.Unlock()
		return nil, err
	}
	return NewUnlocker(lock.Unlock), nil
}

func requirePrivate(paths []string) error {
	for _, path := range paths {
		err := native.EnsurePrivate(path)
		if err == nil {
			continue
		}
		failure, ok := errors.AsType[*native.Error](err)
		switch {
		case !ok:
			return &IOError{Message: err.Error(), Path: path}
		case failure.Errno == syscall.ENOENT:
			continue
		case failure.Errno == syscall.ENOTSUP:
			return &IOError{
				Message: sysx.PathRefusalMessage(syscall.EACCES, "state store must be private to its owner", path),
				Path:    path,
				Errno:   syscall.EACCES,
			}
		}
		return &IOError{Message: failure.Message, Path: failure.Path, Errno: failure.Errno}
	}
	return nil
}

func privateDirectory(path string) error {
	var missing []string
	for probe, ok := path, true; ok; probe, ok = pathx.Parent(probe) {
		if _, err := os.Stat(probe); err == nil {
			break
		}
		missing = append(missing, probe)
	}
	for _, directory := range slices.Backward(missing) {
		if err := os.Mkdir(directory, 0o700); err != nil {
			if errors.Is(err, syscall.EEXIST) {
				continue
			}
			return ioFailure(err, directory)
		}
		if up, ok := pathx.Parent(directory); ok {
			if err := syncDirectory(up); err != nil {
				return err
			}
		}
	}
	return nil
}

func syncDirectory(path string) error {
	directory, err := os.Open(path)
	if err != nil {
		return ioFailure(err, path)
	}
	defer func() { _ = directory.Close() }()
	if err := directory.Sync(); err != nil {
		return ioFailure(err, path)
	}
	return nil
}

func ioFailure(err error, path string) error {
	message, errno := sysx.OSErrorMessage(err, path)
	return &IOError{Message: message, Path: path, Errno: errno}
}

func absolute(path string) (string, error) {
	if path == "" {
		return "", &IOError{Message: "cannot make an empty path absolute"}
	}
	current := ""
	if !pathx.IsAbsolute(path) {
		var err error
		if current, err = sysx.Getwd(); err != nil {
			return "", currentDirectoryFailure(err, path)
		}
	}
	return pathx.Absolute(path, current), nil
}

func currentDirectoryFailure(err error, path string) error {
	return &IOError{Message: sysx.RustOSErrorText(err), Path: path, Errno: sysx.Errno(err)}
}

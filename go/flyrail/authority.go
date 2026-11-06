package flyrail

import (
	"context"
	"errors"
	"io/fs"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/native"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
	"github.com/nielsmadan/flyrail/go/internal/sysx"
)

// ResourceAuthority is the canonical physical destination through which one resource is claimed and locked.
type ResourceAuthority struct {
	destination string
}

// NewResourceAuthority canonicalizes destination on the filesystem; failures are *AuthorityError.
func NewResourceAuthority(ctx context.Context, destination string) (ResourceAuthority, error) {
	canonical, err := CanonicalDestination(ctx, destination)
	if err != nil {
		return ResourceAuthority{}, err
	}
	return ResourceAuthority{destination: canonical}, nil
}

// Destination returns the canonical destination.
func (a ResourceAuthority) Destination() string {
	return a.destination
}

// Identity returns the claim's identity at the authority's canonical destination.
func (c Claim) Identity(a ResourceAuthority) (string, error) {
	if a.destination == "" {
		return "", newValueError("resource authority must come from NewResourceAuthority")
	}
	return c.identity(a.destination)
}

// CanonicalDestination returns the physical spelling of path, made absolute against the working directory, refusing links, special files, hard links, traversal and management state.
func CanonicalDestination(ctx context.Context, path string) (string, error) {
	if err := requireContext(ctx); err != nil {
		return "", err
	}
	if err := ctx.Err(); err != nil {
		return "", &AuthorityError{Message: err.Error(), Err: err}
	}
	if path == "" || strings.IndexByte(path, 0) >= 0 {
		return "", invalidAuthority(newValueError("resource destination must be a nonempty string path without NUL"))
	}
	selected := path
	if !pathx.IsAbsolute(path) {
		current, err := sysx.Getwd()
		if err != nil {
			return "", ioAuthority(err)
		}
		selected = pathx.Join(current, path)
	}
	if slices.Contains(pathx.Components(selected), "..") || !hasParent(selected) {
		return "", invalidAuthority(newValueError("resource destination cannot be a root or contain traversal"))
	}
	if err := validateDestination(selected); err != nil {
		return "", err
	}
	canonical, err := native.CanonicalDestination(selected)
	if err != nil {
		return "", nativeAuthorityError(err)
	}
	if err := validateDestination(canonical); err != nil {
		return "", err
	}
	return canonical, nil
}

func validateDestination(path string) error {
	for _, name := range pathx.Components(path) {
		if name == "/" || name == "." || name == ".." {
			continue
		}
		if !utf8.ValidString(name) {
			return invalidAuthority(surrogateComponentError(name))
		}
		if err := validateRelativePath(name, "resource path component"); err != nil {
			if failure, ok := errors.AsType[*ValueError](err); ok {
				return invalidAuthority(failure)
			}
			return err
		}
		if native.ManagementName(name) {
			return invalidAuthority(newValueError("resource destination enters reserved management state"))
		}
	}
	return nil
}

func surrogateComponentError(name string) *ValueError {
	for cursor := 0; cursor < len(name); {
		lead := name[cursor]
		width := 0
		switch {
		case lead <= 0x7f:
			width = 1
		case lead >= 0xc0 && lead <= 0xdf:
			width = 2
		case lead >= 0xe0 && lead <= 0xef:
			width = 3
		case lead >= 0xf0 && lead <= 0xf7:
			width = 4
		}
		if width == 0 || cursor+width > len(name) || !utf8.ValidString(name[cursor:cursor+width]) {
			cursor++
			continue
		}
		code, _ := utf8.DecodeRuneInString(name[cursor:])
		if code < 0x20 || (code >= 0x7f && code <= 0x9f) || strings.IndexByte(`<>:"\|?*`, lead) >= 0 {
			return newValueError("resource path component contains a nonportable character")
		}
		cursor += width
	}
	return newValueError("resource path component must contain NFC-normalized Unicode scalar values")
}

func invalidAuthority(err *ValueError) *AuthorityError {
	return &AuthorityError{Message: err.Message, Err: err}
}

func ioAuthority(err error) *AuthorityError {
	message, errno := sysx.OSErrorMessageWithoutPath(err)
	if errno != 0 {
		return &AuthorityError{Message: message, Err: errno}
	}
	return &AuthorityError{Message: message, Err: err}
}

func nativeAuthorityError(err error) *AuthorityError {
	failure, ok := errors.AsType[*native.Error](err)
	if !ok {
		return &AuthorityError{Message: err.Error(), Err: err}
	}
	switch failure.Kind {
	case native.KindInvalid:
		return invalidAuthority(newValueError(failure.Message))
	case native.KindUnsafe:
		return &AuthorityError{Message: failure.Message, Path: failure.Path, Err: ErrUnsafePath}
	}
	return &AuthorityError{Message: failure.Message, Path: failure.Path, Err: nativeCause(failure)}
}

func nativeCause(failure *native.Error) error {
	switch {
	case failure.Errno != 0 && failure.Path != "":
		return &fs.PathError{Op: "resolve", Path: failure.Path, Err: failure.Errno}
	case failure.Errno != 0:
		return failure.Errno
	case failure.Kind == native.KindUnsupported:
		return errors.ErrUnsupported
	}
	return errors.New(failure.Message)
}

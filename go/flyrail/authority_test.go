//go:build darwin || linux

package flyrail

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"syscall"
	"testing"

	"github.com/nielsmadan/flyrail/go/internal/native"
)

const testTransaction = "0123456789abcdef0123456789abcdef"

func authorityMessage(t *testing.T, path string) string {
	t.Helper()
	_, err := CanonicalDestination(t.Context(), path)
	failure, ok := errors.AsType[*AuthorityError](err)
	if !ok {
		t.Fatalf("CanonicalDestination(%q) error = %v, want *AuthorityError", path, err)
	}
	return failure.Message
}

func authority(t *testing.T, path string) ResourceAuthority {
	t.Helper()
	return must(NewResourceAuthority(t.Context(), path))
}

func directoryNames(t *testing.T, path string) []string {
	t.Helper()
	entries, err := os.ReadDir(path)
	if err != nil {
		t.Fatal(err)
	}
	names := make([]string, 0, len(entries))
	for _, entry := range entries {
		names = append(names, entry.Name())
	}
	return names
}

func TestCanonicalDestinationMessagesMatchPython(t *testing.T) {
	t.Parallel()
	for _, test := range []struct{ path, want string }{
		{"", "resource destination must be a nonempty string path without NUL"},
		{"/a\x00b", "resource destination must be a nonempty string path without NUL"},
		{"/", "resource destination cannot be a root or contain traversal"},
		{"/a/../b", "resource destination cannot be a root or contain traversal"},
		{"../other", "resource destination cannot be a root or contain traversal"},
		{"/tmp/.FLYRAIL-x.TXN", "resource destination enters reserved management state"},
		{"/tmp/CON", "resource path component contains a reserved Windows name"},
		{"/tmp/a<b", "resource path component contains a nonportable character"},
		{"/tmp/\xff", "resource path component must contain NFC-normalized Unicode scalar values"},
		{"/tmp/a\xc2\x85\xff", "resource path component contains a nonportable character"},
		{"rel\xff", "resource path component must contain NFC-normalized Unicode scalar values"},
		{"/tmp/\x00\xff", "resource destination must be a nonempty string path without NUL"},
		{"/tmp/\xff/../b", "resource destination cannot be a root or contain traversal"},
		{"/tmp/.flyrail-x.txn/\xff", "resource destination enters reserved management state"},
	} {
		if got := authorityMessage(t, test.path); got != test.want {
			t.Errorf("CanonicalDestination(%q) = %q, want %q", test.path, got, test.want)
		}
	}
}

func TestAuthorityErrorsCarryTheirCause(t *testing.T) {
	t.Parallel()
	_, err := CanonicalDestination(t.Context(), "/")
	if _, ok := errors.AsType[*ValueError](err); !ok {
		t.Errorf("invalid destination error = %v, want a *ValueError cause", err)
	}
	tmp := tempRoot(t)
	link := filepath.Join(tmp, "link")
	if err := os.Symlink(tmp, link); err != nil {
		t.Fatal(err)
	}
	_, err = CanonicalDestination(t.Context(), filepath.Join(link, "file"))
	failure, ok := errors.AsType[*AuthorityError](err)
	if !ok || !errors.Is(err, ErrUnsafePath) || failure.Path != link ||
		failure.Message != "resource links and special files are unsupported: "+link {
		t.Errorf("link destination error = %#v", err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if _, err := CanonicalDestination(ctx, filepath.Join(tmp, "file")); !errors.Is(err, context.Canceled) {
		t.Errorf("canceled context error = %v", err)
	}
}

func TestCanonicalDestinationReportsPythonOSErrorText(t *testing.T) {
	t.Parallel()
	locked := filepath.Join(tempRoot(t), "locked")
	if err := os.Mkdir(locked, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(locked, 0); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(locked, 0o700) })
	_, err := CanonicalDestination(t.Context(), filepath.Join(locked, "child.md"))
	failure, ok := errors.AsType[*AuthorityError](err)
	want := "[Errno 13] Permission denied: '" + locked + "'"
	if !ok || failure.Message != want || failure.Path != locked || !errors.Is(err, syscall.EACCES) {
		t.Errorf("error = %#v, want %q at %s", err, want, locked)
	}
}

func TestRelativeDestinationsSnapshotThePhysicalWorkingDirectory(t *testing.T) {
	t.Parallel()
	current, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	physical := must(native.PhysicalDirectory(current))
	if got := must(CanonicalDestination(t.Context(), "flyrail-probe/rel")); got != physical+"/flyrail-probe/rel" {
		t.Errorf("relative destination = %q, want %q", got, physical+"/flyrail-probe/rel")
	}
}

func TestRoutesToTheSameDocumentShareOneAuthorityWithoutWrites(t *testing.T) {
	t.Parallel()
	tmp := tempRoot(t)
	project := filepath.Join(tmp, "home/work/project")
	if err := os.MkdirAll(project, 0o700); err != nil {
		t.Fatal(err)
	}
	document := filepath.Join(project, "AGENTS.md")
	if err := os.WriteFile(document, []byte("owned and foreign text"), 0o600); err != nil {
		t.Fatal(err)
	}
	fromHome := authority(t, filepath.Join(tmp, "home")+"/work/./project//AGENTS.md")
	fromProject := authority(t, document)
	if fromHome != fromProject || fromHome.Destination() != document {
		t.Errorf("authorities = %q and %q, want %q", fromHome.Destination(), fromProject.Destination(), document)
	}
	if parent := filepath.Dir(must(TransactionPath(fromHome.Destination(), testTransaction))); parent != project {
		t.Errorf("transaction directory parent = %q, want %q", parent, project)
	}
	if names := directoryNames(t, project); !slices.Equal(names, []string{"AGENTS.md"}) {
		t.Errorf("project entries = %v", names)
	}
}

func TestAbsentDestinationsKeepTheirSpelling(t *testing.T) {
	t.Parallel()
	tmp := tempRoot(t)
	home := authority(t, filepath.Join(tmp, "home/AGENTS.md"))
	project := authority(t, filepath.Join(tmp, "home/project/AGENTS.md"))
	if home.Destination() != filepath.Join(tmp, "home/AGENTS.md") {
		t.Errorf("home destination = %q", home.Destination())
	}
	if must(TransactionPath(project.Destination(), testTransaction)) == must(TransactionPath(home.Destination(), testTransaction)) {
		t.Errorf("nested destinations share a transaction path")
	}
	if names := directoryNames(t, tmp); len(names) != 0 {
		t.Errorf("authority created entries %v", names)
	}
}

func TestLongResourceNamesHaveBoundedSiblingStateNames(t *testing.T) {
	t.Parallel()
	destination := authority(t, filepath.Join(tempRoot(t), strings.Repeat("a", 240))).Destination()
	if name := filepath.Base(must(TransactionPath(destination, testTransaction))); len(name) != 110 {
		t.Errorf("transaction name length = %d, want 110", len(name))
	}
}

func TestAncestorCaseAliasesPreserveClaimIdentity(t *testing.T) {
	t.Parallel()
	tmp := tempRoot(t)
	parent := filepath.Join(tmp, "Config")
	if err := os.Mkdir(parent, 0o700); err != nil {
		t.Fatal(err)
	}
	alias := filepath.Join(tmp, "config")
	if info, err := os.Stat(alias); err != nil || !info.IsDir() {
		t.Skip("host is case sensitive")
	}
	first := authority(t, filepath.Join(parent, "absent/config.json"))
	second := authority(t, filepath.Join(alias, "absent/config.json"))
	if first != second {
		t.Errorf("authorities = %q and %q", first.Destination(), second.Destination())
	}
	claim := must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("mcp"))))))
	if must(claim.Identity(first)) != must(claim.Identity(second)) {
		t.Errorf("case aliases changed the claim identity")
	}
}

func TestResourceCannotEnterManagementStateOrARegularAncestor(t *testing.T) {
	t.Parallel()
	tmp := tempRoot(t)
	if got := authorityMessage(t, filepath.Join(tmp, ".flyrail-deadbeef.txn/staging")); got != "resource destination enters reserved management state" {
		t.Errorf("management destination = %q", got)
	}
	for _, name := range []string{".flyrail-deadbeef.txn", ".FLYRAIL-deadbeef.TXN"} {
		for _, destination := range []string{filepath.Join(tmp, name), filepath.Join(tmp, name, "staging")} {
			if got := authorityMessage(t, destination); got != "resource destination enters reserved management state" {
				t.Errorf("%s = %q", destination, got)
			}
		}
	}
	file := filepath.Join(tmp, "file")
	if err := os.WriteFile(file, []byte("file"), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := authorityMessage(t, filepath.Join(file, "child")); got != "resource ancestor must be a directory" {
		t.Errorf("regular ancestor = %q", got)
	}
	if got := authority(t, file).Destination(); got != file {
		t.Errorf("file authority = %q", got)
	}
}

func TestCanonicalPhysicalPathCannotEnterManagementState(t *testing.T) {
	t.Parallel()
	physical := filepath.Join(tempRoot(t), ".FLYRAIL-deadbeef.TXN")
	if err := os.Mkdir(physical, 0o700); err != nil {
		t.Fatal(err)
	}
	direct := must(native.CanonicalChild(physical, "receipt.json"))
	nested := must(native.CanonicalChild(physical, "absent")) + "/receipt.json"
	for _, path := range []string{direct, nested} {
		err := validateDestination(path)
		if err == nil || err.Error() != "resource destination enters reserved management state" {
			t.Errorf("validateDestination(%q) = %v", path, err)
		}
	}
}

func TestCaseAliasesShareAuthorityOnlyOnCaseInsensitiveHosts(t *testing.T) {
	t.Parallel()
	tmp := tempRoot(t)
	original := filepath.Join(tmp, "AGENTS.md")
	if err := os.WriteFile(original, []byte("text"), 0o600); err != nil {
		t.Fatal(err)
	}
	alias := filepath.Join(tmp, "agents.md")
	if _, err := os.Stat(alias); err == nil {
		if authority(t, alias) != authority(t, original) {
			t.Errorf("case aliases have different authorities")
		}
		return
	}
	if got := authorityMessage(t, alias); got != "resource path has a conflicting portable spelling" {
		t.Errorf("alias = %q", got)
	}
	if err := os.WriteFile(alias, []byte("different file"), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := authorityMessage(t, original); got != "portable resource path collision" {
		t.Errorf("collision = %q", got)
	}
}

func TestHardLinkedDocumentsCannotCreateSplitAuthorities(t *testing.T) {
	t.Parallel()
	tmp := tempRoot(t)
	first, second := filepath.Join(tmp, "first.md"), filepath.Join(tmp, "second.md")
	if err := os.WriteFile(first, []byte("same inode"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(first, second); err != nil {
		t.Skip("host does not permit hard links")
	}
	for _, path := range []string{first, second} {
		if got := authorityMessage(t, path); got != "hard-linked resources are ambiguous: "+path {
			t.Errorf("%s = %q", path, got)
		}
	}
}

func TestClaimIdentityUsesThePhysicalSelectorNotAttribution(t *testing.T) {
	t.Parallel()
	at := authority(t, filepath.Join(tempRoot(t), "AGENTS.md"))
	section := func(start, end string) Claim {
		return must(NewClaim(OwnershipSection, must(NewSectionBoundaries(start, end))))
	}
	first := section("start", "end")
	if !first.Overlaps(section("start", "other end")) || !first.Overlaps(section("end", "other end")) {
		t.Errorf("section claims sharing a marker do not overlap")
	}
	if must(first.Identity(at)) != must(section("start", "end").Identity(at)) {
		t.Errorf("equal section claims have different identities")
	}
	if must(first.Identity(at)) == must(section("new start", "new end").Identity(at)) {
		t.Errorf("moved section claim kept its identity")
	}
	member := func(identity Value) Claim {
		return must(NewClaim(OwnershipStructured, must(NewSelector(must(NewKey("hooks")), must(NewMember(identity, nil))))))
	}
	if must(member(Int(1)).Identity(at)) == must(member(Bool(true)).Identity(at)) {
		t.Errorf("integer and boolean member identities collide")
	}
	if want := must(first.identity(at.Destination())); must(first.Identity(at)) != want {
		t.Errorf("Identity differs from the identity at the canonical destination")
	}
}

func TestClaimIdentityRequiresAnAuthority(t *testing.T) {
	t.Parallel()
	_, err := must(NewClaim(OwnershipFile, WholeSelector{})).Identity(ResourceAuthority{})
	if failure, ok := errors.AsType[*ValueError](err); !ok || failure.Message != "resource authority must come from NewResourceAuthority" {
		t.Errorf("Identity(zero authority) = %v", err)
	}
}

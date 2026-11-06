package native

import (
	"strings"
	"testing"
)

const (
	rootMount = "22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw"
	homeMount = "30 22 8:2 / /home rw,relatime shared:2 - ext4 /dev/sda2 rw"
)

func mountInfo(lines ...string) string {
	var out strings.Builder
	for _, line := range lines {
		out.WriteString(line + "\n")
	}
	return out.String()
}

func TestSharedMount(t *testing.T) {
	bound := mountInfo(rootMount, homeMount,
		"41 30 8:2 /u/.state /work/state rw,relatime - ext4 /dev/sda2 rw",
		"42 30 8:2 /u/project /work/project rw,relatime - ext4 /dev/sda2 rw")
	escaped := mountInfo(rootMount, homeMount, `43 30 8:2 /x /home/u/my\040state rw - ext4 /dev/sda2 rw`)
	stacked := mountInfo(rootMount, homeMount, "44 30 8:3 / /home rw - tmpfs tmpfs rw")
	cases := []struct {
		name          string
		info          string
		first, second string
		want          bool
	}{
		{"same entry", mountInfo(rootMount, homeMount), "/home/u/.state/staging", "/home/u/project", true},
		{"different entries", mountInfo(rootMount, homeMount), "/home/u/.state/staging", "/srv/project", false},
		{"bind mounts of one device", bound, "/work/state/staging", "/work/project", false},
		{"inside one bind mount", bound, "/work/project/a", "/work/project/b", true},
		{"escaped mount point apart", escaped, "/home/u/my state/staging", "/home/u/project", false},
		{"escaped mount point shared", escaped, "/home/u/my state/staging", "/home/u/my state/p", true},
		{"stacked mount hides the lower", stacked, "/home/u/staging", "/", false},
		{"stacked mount shared", stacked, "/home/u/staging", "/home/v", true},
		{"component prefix only", mountInfo(rootMount, homeMount), "/homes/a", "/home/a", false},
		{"no covering entry", mountInfo(homeMount), "/srv/state", "/srv/project", false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := SharedMount([]byte(c.info), c.first, c.second); got != c.want {
				t.Fatalf("SharedMount(%q, %q) = %v, want %v", c.first, c.second, got, c.want)
			}
		})
	}
}

func TestDoubtfulMountInformationSharesNoMount(t *testing.T) {
	cases := []string{
		"",
		"garbage\n",
		mountInfo(rootMount, "30 22 8:2 / /home rw,relatime shared:2 ext4 /dev/sda2 rw"),
		mountInfo(rootMount, "x 22 8:2 / /home rw - ext4 /dev/sda2 rw"),
		mountInfo(rootMount, "30 22 8-2 / /home rw - ext4 /dev/sda2 rw"),
		mountInfo(rootMount, "", homeMount),
		mountInfo(rootMount, "30 22 8:2 / home rw - ext4 /dev/sda2 rw"),
		mountInfo(rootMount, "99999999999999999999 22 8:2 / /home rw - ext4 /dev/sda2 rw"),
		mountInfo(rootMount, homeMount) + "\r",
		mountInfo(rootMount, homeMount) + " ",
	}
	for _, info := range cases {
		if SharedMount([]byte(info), "/home/u/.state/staging", "/home/u/project") {
			t.Errorf("SharedMount(%q) = true, want false", info)
		}
	}
}

func TestAMountPointThatIsNotUTF8SharesNoMount(t *testing.T) {
	info := []byte(mountInfo(rootMount, homeMount) + "45 30 8:2 /x /home/u/st\xffate rw - ext4 /dev/sda2 rw\n")
	if SharedMount(info, "/home/u/st\xffate/staging", "/home/u/project") {
		t.Error("non-UTF-8 staging path shares a mount")
	}
	if SharedMount(info, "/home/u/.state/staging", "/home/u/project") {
		t.Error("UTF-8 paths share a mount in a non-UTF-8 table")
	}
}

func TestMountPointDecodesOctalEscapes(t *testing.T) {
	cases := map[string]string{
		`/a\040b`:  "/a b",
		`/a\134`:   `/a\`,
		`/a\011b`:  "/a\tb",
		`/a\400`:   `/a\400`,
		`/a\04`:    `/a\04`,
		`/a\08x`:   `/a\08x`,
		`/a\377z`:  "/a\xffz",
		`/plain`:   "/plain",
		`/a\\040b`: `/a\ b`,
	}
	for field, want := range cases {
		if got := mountPoint(field); got != want {
			t.Errorf("mountPoint(%q) = %q, want %q", field, got, want)
		}
	}
}

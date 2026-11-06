package native

import (
	"testing"
	"time"
)

func TestChangedAfterComparesBirthOnlyWhenKnown(t *testing.T) {
	boundary := time.Unix(1_000_000, 0)
	born := boundary.Add(time.Minute)
	cases := []struct {
		name  string
		times Times
		want  bool
	}{
		{"born after the boundary", Times{Modified: boundary, Born: born, HasBorn: true}, true},
		{"birth not compared", Times{Modified: boundary, Born: born}, false},
		{"modified after the boundary", Times{Modified: born}, true},
		{"both before", Times{Modified: boundary.Add(-time.Minute), Born: boundary.Add(-time.Hour), HasBorn: true}, false},
	}
	for _, c := range cases {
		if got := c.times.ChangedAfter(boundary); got != c.want {
			t.Errorf("%s: ChangedAfter = %v, want %v", c.name, got, c.want)
		}
	}
}

package seams

import (
	"context"
	"testing"

	"github.com/google/go-cmp/cmp"
)

func TestHooksTravelInTheContext(t *testing.T) {
	t.Parallel()
	var events []Event
	hooks := &Hooks{
		Observe: func(event Event) { events = append(events, event) },
		Fault:   func(boundary string) bool { return boundary == Journal },
	}
	ctx := With(t.Context(), hooks)
	Fire(ctx, Event{Kind: KindRename, Source: "/a", Destination: "/b"})
	if Inject(ctx, Staged) {
		t.Errorf("Inject(%q) = true, want false", Staged)
	}
	if !Inject(ctx, Journal) {
		t.Errorf("Inject(%q) = false, want true", Journal)
	}
	want := []Event{
		{Kind: KindRename, Source: "/a", Destination: "/b"},
		{Kind: KindBoundary, Boundary: Staged},
		{Kind: KindBoundary, Boundary: Journal},
	}
	if diff := cmp.Diff(want, events); diff != "" {
		t.Errorf("events (-want +got):\n%s", diff)
	}
}

func TestContextsWithoutHooksInjectNothing(t *testing.T) {
	t.Parallel()
	if Inject(context.Background(), Journal) {
		t.Error("Inject without hooks = true")
	}
	if Inject(With(t.Context(), &Hooks{}), Journal) {
		t.Error("Inject with empty hooks = true")
	}
	var nilContext context.Context
	if Inject(nilContext, Journal) {
		t.Error("Inject with a nil context = true")
	}
}

package seams

import "context"

// Kind classifies a seam event.
type Kind uint8

// The seam event kinds, as Rust's seam::Seam variants.
const (
	KindBoundary Kind = iota + 1
	KindRename
	KindUnlink
)

// The named transaction boundaries, spelled as Rust's resource_transaction inject points.
const (
	Preparation          = "preparation"
	Prepare              = "prepare"
	Directory            = "directory"
	Staged               = "staged"
	Journal              = "journal"
	PublishReceipt       = "publish-receipt"
	Receipt              = "receipt"
	ReceiptReread        = "receipt-reread"
	ReceiptCommitted     = "receipt-committed"
	BackupMove           = "backup-move"
	Backup               = "backup"
	Rename               = "rename"
	Staging              = "staging"
	Publish              = "publish"
	Cleanup              = "cleanup"
	Rollback             = "rollback"
	ResourcePrecondition = "resource-precondition"
	IndexRequire         = "index-require"
	GuardLookup          = "guard-lookup"
	IndexCompletion      = "index-completion"
)

// Event is one seam firing: a named boundary, an exclusive rename before or after it happens, or a file unlink.
type Event struct {
	Kind        Kind
	Boundary    string
	Source      string
	Destination string
	Done        bool
	Path        string
}

// Hooks are the test callbacks for one operation; Observe runs synchronously at every event and may block as a barrier, and Fault reports whether to inject a failure at a boundary, and Fail returns the error to surface there.
type Hooks struct {
	Observe func(Event)
	Fault   func(boundary string) bool
	Fail    func(boundary string) error
}

// BarrierTimedOut is the Fail hook's error when a barrier's release file never appeared.
type BarrierTimedOut struct{ Path string }

func (BarrierTimedOut) Error() string { return "interop barrier timed out" }

type contextKey struct{}

// With returns a context carrying hooks.
func With(ctx context.Context, hooks *Hooks) context.Context {
	return context.WithValue(ctx, contextKey{}, hooks)
}

func from(ctx context.Context) *Hooks {
	if ctx == nil {
		return nil
	}
	hooks, _ := ctx.Value(contextKey{}).(*Hooks)
	return hooks
}

// Fire delivers event to the context's Observe hook, if any.
func Fire(ctx context.Context, event Event) {
	if hooks := from(ctx); hooks != nil && hooks.Observe != nil {
		hooks.Observe(event)
	}
}

// Inject fires the boundary event and reports whether the context's Fault hook injects a failure there.
func Inject(ctx context.Context, boundary string) bool {
	Fire(ctx, Event{Kind: KindBoundary, Boundary: boundary})
	hooks := from(ctx)
	return hooks != nil && hooks.Fault != nil && hooks.Fault(boundary)
}

// Failure returns the error the context's Fail hook raises at boundary, if any.
func Failure(ctx context.Context, boundary string) error {
	if hooks := from(ctx); hooks != nil && hooks.Fail != nil {
		return hooks.Fail(boundary)
	}
	return nil
}

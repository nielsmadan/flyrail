package flyrail

import (
	"crypto/sha256"
	"encoding/hex"
	"maps"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/nielsmadan/flyrail/go/internal/jsonx"
	"github.com/nielsmadan/flyrail/go/internal/pathx"
)

// ArtifactTranslation is one translated artifact, or none, with the notices its translation produced.
type ArtifactTranslation struct {
	artifact    RenderedArtifact
	hasArtifact bool
	notices     []Notice
}

// Artifact returns the translated artifact, if the request could be translated.
func (t ArtifactTranslation) Artifact() (RenderedArtifact, bool) {
	return t.artifact, t.hasArtifact
}

// Notices returns a copy of the translation notices.
func (t ArtifactTranslation) Notices() []Notice {
	return slices.Clone(t.notices)
}

// Supported reports whether an artifact was produced and no notice marks the request unsupported.
func (t ArtifactTranslation) Supported() bool {
	return t.hasArtifact && !blocking(t.notices)
}

func identifier(values ...string) string {
	items := make([]jsonx.Value, len(values))
	for index, value := range values {
		items[index] = jsonx.String(value)
	}
	sum := sha256.Sum256(jsonx.Encode(jsonx.Array(items...)))
	return hex.EncodeToString(sum[:])
}

func unsupportedTranslationResult(artifactID string, failure *UnsupportedTranslationError) ArtifactTranslation {
	return ArtifactTranslation{notices: []Notice{{
		ArtifactID: artifactID,
		Kind:       NoticeUnsupported,
		Message:    failure.Message,
		Code:       failure.Code,
	}}}
}

func translationNotices(artifactID string, family Family, rc RenderContext) []Notice {
	var notices []Notice
	add := func(kind NoticeKind, message, code string) {
		notices = append(notices, Notice{ArtifactID: artifactID, Kind: kind, Message: message, Code: code})
	}
	agent := rc.audience.agent
	if family == FamilyInstructions {
		add(NoticeActivation, "Instruction discovery and loading are controlled by the host and current session.", "instruction-loading")
		if agent == AgentCodex {
			add(NoticeActivation, "AGENTS.override.md takes precedence; instructions default to a 32 KiB limit. Start a new session after changes and check host discovery.", "codex-instruction-discovery")
		}
		if rc.surface == SurfaceVSCode && rc.target.scope == ScopeUser {
			add(NoticeActivation, "User instructions target VS Code Agent Host; local extension-host sessions may use different discovery settings.", "vscode-agent-host-instructions")
		}
	}
	if family == FamilyMCP {
		add(NoticeActivation, "Registration does not grant trust, approve tools, supply credentials or prove server activation.", "mcp-activation")
		if agent == AgentPi {
			add(NoticePrerequisite, "Requires externally installed pi-mcp-adapter; emitted schema targets v2.32.1.", "pi-mcp-adapter-v2-32-1")
		}
		if rc.surface == SurfaceVSCode {
			add(NoticeActivation, "VS Code forwards these servers to Agent Host, which does not read this servers document directly.", "vscode-mcp-forwarding")
		}
	}
	return notices
}

// TranslateInstructionArtifact translates an instruction for rc, marking its section with a digest of bundleID and the artifact id.
func TranslateInstructionArtifact(bundleID string, a InstructionArtifact, rc RenderContext) (ArtifactTranslation, error) {
	if err := rc.check(); err != nil {
		return ArtifactTranslation{}, err
	}
	if !utf8.ValidString(bundleID) {
		return ArtifactTranslation{}, newValueError("bundle id must contain Unicode scalar values")
	}
	if err := checkArtifact(a); err != nil {
		return ArtifactTranslation{}, err
	}
	return translateInstruction(bundleID, a, rc)
}

func translateInstruction(bundleID string, a InstructionArtifact, rc RenderContext) (ArtifactTranslation, error) {
	if failure := requireTranslatable(rc); failure != nil {
		return unsupportedTranslationResult(a.id, failure), nil
	}
	path, err := instructionDestination(rc)
	if err != nil {
		return ArtifactTranslation{}, err
	}
	if path == "" {
		return unsupportedTranslationResult(a.id, unsupportedTranslation("instruction-destination",
			"Cursor user rules have no verified filesystem preset.")), nil
	}
	marker := identifier(bundleID, a.id)
	var payload Content
	if rc.surface == SurfaceVSCode && rc.target.scope == ScopeUser {
		if rc.instructionPath == "" {
			path = pathx.Join(path, marker+".instructions.md")
		}
		if name, ok := pathx.FileName(path); !ok || !utf8.ValidString(name) || !strings.HasSuffix(name, ".instructions.md") {
			return unsupportedTranslationResult(a.id, unsupportedTranslation("instruction-extension",
				"VS Code user instructions require an .instructions.md file.")), nil
		}
		payload = FileContent{data: []byte("---\napplyTo: \"**\"\n---\n\n" + a.text), mode: ModePrivate}
	} else {
		name, _ := pathx.FileName(path)
		if !utf8.ValidString(name) {
			name = ""
		}
		if extension, ok := pathExtension(path); ok && extension == "mdc" || strings.HasSuffix(name, ".instructions.md") {
			return unsupportedTranslationResult(a.id, unsupportedTranslation("native-instruction-required",
				"Use native content for authored modular rule metadata.")), nil
		}
		section, err := NewSectionContent(marker, a.text, nil)
		if err != nil {
			return ArtifactTranslation{}, err
		}
		payload = section
	}
	artifact, err := NewRenderedArtifact(RenderedArtifactParams{ID: a.id, Family: FamilyInstructions, Destination: path, Content: payload})
	if err != nil {
		return ArtifactTranslation{}, err
	}
	return ArtifactTranslation{artifact: artifact, hasArtifact: true, notices: translationNotices(a.id, FamilyInstructions, rc)}, nil
}

// TranslateMCPArtifact translates an MCP registration for rc; assetDestinations maps asset ids to absolute rendered destinations.
func TranslateMCPArtifact(a MCPArtifact, rc RenderContext, assetDestinations map[string]string) (ArtifactTranslation, error) {
	if err := rc.check(); err != nil {
		return ArtifactTranslation{}, err
	}
	if err := checkArtifact(a); err != nil {
		return ArtifactTranslation{}, err
	}
	return translateMCP(a, rc, maps.Clone(assetDestinations))
}

func translateMCP(a MCPArtifact, rc RenderContext, assets map[string]string) (ArtifactTranslation, error) {
	if failure := requireTranslatable(rc); failure != nil {
		return unsupportedTranslationResult(a.id, failure), nil
	}
	path, err := mcpDestination(rc)
	if err != nil {
		return ArtifactTranslation{}, err
	}
	if path == "" {
		if rc.surface == SurfaceVSCode {
			return unsupportedTranslationResult(a.id, unsupportedTranslation("mcp-profile-path",
				"VS Code user MCP requires an explicit profile mcp.json path.")), nil
		}
		return unsupportedTranslationResult(a.id, unsupportedTranslation("mcp-explicit-path",
			"Relocated Claude user MCP requires an explicit mcp_path.")), nil
	}
	if name, _ := pathx.FileName(path); rc.surface == SurfaceVSCode && name != "mcp.json" {
		return unsupportedTranslationResult(a.id, unsupportedTranslation("mcp-profile-schema",
			"VS Code routes require mcp.json with the servers schema; use the CLI surface for Agent Host native mcpServers files.")), nil
	}
	payload, failure := mcpContent(a, rc, path, assets)
	if failure != nil {
		if failure.unsupported != nil {
			return unsupportedTranslationResult(a.id, failure.unsupported), nil
		}
		return ArtifactTranslation{}, failure.invalid
	}
	artifact, err := NewRenderedArtifact(RenderedArtifactParams{ID: a.id, Family: FamilyMCP, Destination: path, Content: payload})
	if err != nil {
		return ArtifactTranslation{}, err
	}
	return ArtifactTranslation{artifact: artifact, hasArtifact: true, notices: translationNotices(a.id, FamilyMCP, rc)}, nil
}

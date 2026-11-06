package main

import (
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"errors"
	"io"

	"github.com/nielsmadan/flyrail/go/flyrail"
)

const (
	bundleID       = "example-checksum"
	configBundleID = "example-checksum-config"
	guidance       = "Use flyrail-checksum digest FILE to verify downloaded files.\n"
)

var (
	//go:embed resources/flyrail/flyrail.json
	manifest []byte
	//go:embed resources/flyrail/skills/checksum-review/SKILL.md
	skillText []byte
	//go:embed resources/flyrail/skills/checksum-review/assets/sample.bin
	sample []byte
	//go:embed resources/flyrail/skills/checksum-review/scripts/verify_checksum.py
	verifyScript []byte
	//go:embed resources/flyrail/skills/checksum-review/scratch/README.txt
	scratchReadme []byte
	//go:embed resources/assets/reminder.py
	reminderScript []byte
	//go:embed resources/assets/reminder.txt
	reminderText []byte
)

func sha256Reader(r io.Reader) (string, error) {
	digest := sha256.New()
	if _, err := io.Copy(digest, r); err != nil {
		return "", err
	}
	return hex.EncodeToString(digest.Sum(nil)), nil
}

// Executable intent is declared on the entry and in the manifest, never read from file modes.
func packagedBundle() (*flyrail.Bundle, error) {
	return flyrail.NewBundleFromEntries(manifest, []flyrail.BundleEntry{
		{Path: "skills/checksum-review", Dir: true},
		{Path: "skills/checksum-review/SKILL.md", Data: skillText},
		{Path: "skills/checksum-review/assets", Dir: true},
		{Path: "skills/checksum-review/assets/sample.bin", Data: sample},
		{Path: "skills/checksum-review/scripts", Dir: true},
		{Path: "skills/checksum-review/scripts/verify_checksum.py", Data: verifyScript, Executable: true},
		{Path: "skills/checksum-review/scratch", Dir: true},
		{Path: "skills/checksum-review/scratch/README.txt", Data: scratchReadme},
	})
}

func configurationBundle(text string, mcpArgv []string, mcpURL string) (*flyrail.Bundle, error) {
	skills, err := packagedBundle()
	if err != nil {
		return nil, err
	}
	if skills.ID() != bundleID {
		return nil, errors.New("packaged bundle ID differs from the host's stable ID")
	}
	instruction, err := flyrail.NewInstructionArtifact("guidance", text)
	if err != nil {
		return nil, err
	}
	local, err := localMCP(mcpArgv)
	if err != nil {
		return nil, err
	}
	remote, err := remoteMCP(mcpURL)
	if err != nil {
		return nil, err
	}
	hook, err := reminderHook()
	if err != nil {
		return nil, err
	}
	assets, err := flyrail.NewTreeContent([]flyrail.BundleEntry{
		{Path: "reminder.py", Data: reminderScript},
		{Path: "reminder.txt", Data: reminderText},
	})
	if err != nil {
		return nil, err
	}
	asset, err := flyrail.NewSupportAsset("reminder-assets", flyrail.FamilyHooks, assets)
	if err != nil {
		return nil, err
	}
	dependency, err := flyrail.NewDependency("reminder", "reminder-assets", flyrail.DependencyRevision)
	if err != nil {
		return nil, err
	}
	identity, err := flyrail.NewBundleIdentity(configBundleID, skills.Version())
	if err != nil {
		return nil, err
	}
	artifacts := append(skills.Artifacts(), instruction, local, remote, hook)
	return flyrail.NewBundle(identity, artifacts, []flyrail.SupportAsset{asset}, []flyrail.Dependency{dependency})
}

func localMCP(argv []string) (flyrail.MCPArtifact, error) {
	token, err := flyrail.NewEnvRef("CHECKSUM_MCP_TOKEN")
	if err != nil {
		return flyrail.MCPArtifact{}, err
	}
	arguments := make([]flyrail.CommandArgument, 0, len(argv))
	for _, argument := range argv {
		arguments = append(arguments, flyrail.ArgText(argument))
	}
	command, err := flyrail.NewCommand(arguments, map[string]flyrail.EnvironmentValue{"CHECKSUM_MCP_TOKEN": token}, nil)
	if err != nil {
		return flyrail.MCPArtifact{}, err
	}
	return flyrail.NewMCPArtifact("local", "checksum-local", command)
}

func remoteMCP(url string) (flyrail.MCPArtifact, error) {
	token, err := flyrail.NewEnvRef("CHECKSUM_HTTP_TOKEN")
	if err != nil {
		return flyrail.MCPArtifact{}, err
	}
	transport, err := flyrail.NewHTTPTransport(url, &token)
	if err != nil {
		return flyrail.MCPArtifact{}, err
	}
	return flyrail.NewMCPArtifact("remote", "checksum-remote", transport)
}

func reminderHook() (flyrail.HookArtifact, error) {
	script, err := flyrail.NewAssetRef("reminder-assets", "reminder.py")
	if err != nil {
		return flyrail.HookArtifact{}, err
	}
	command, err := flyrail.NewCommand([]flyrail.CommandArgument{flyrail.ArgText("python3"), script}, nil, nil)
	if err != nil {
		return flyrail.HookArtifact{}, err
	}
	return flyrail.NewHookArtifact(flyrail.HookArtifactParams{
		ID:            "reminder",
		Event:         flyrail.EventBeforeTool,
		Command:       command,
		TimeoutMillis: 10_000,
		Outcomes:      []flyrail.HookOutcome{flyrail.OutcomeContext},
	})
}

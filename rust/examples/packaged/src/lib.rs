use flyrail::{
    Artifact, AssetRef, Bundle, BundleEntry, BundleError, BundleIdentity, Command, Dependency,
    EnvRef, Family, HookArtifact, HookEvent, HookOutcome, HttpTransport, InstructionArtifact,
    McpArtifact, McpTransport, SupportAsset, TreeContent,
};
use sha2::{Digest as _, Sha256};
use std::io::{self, Read};

pub const BUNDLE_ID: &str = "example-checksum";
pub const CONFIG_BUNDLE_ID: &str = "example-checksum-config";
pub const GUIDANCE: &str = "Use flyrail-checksum digest FILE to verify downloaded files.\n";

const MANIFEST: &[u8] = include_bytes!("../resources/flyrail/flyrail.json");
const SKILL: &[u8] = include_bytes!("../resources/flyrail/skills/checksum-review/SKILL.md");
const SAMPLE: &[u8] =
    include_bytes!("../resources/flyrail/skills/checksum-review/assets/sample.bin");
const VERIFY: &[u8] =
    include_bytes!("../resources/flyrail/skills/checksum-review/scripts/verify_checksum.py");
const SCRATCH: &[u8] =
    include_bytes!("../resources/flyrail/skills/checksum-review/scratch/README.txt");
const REMINDER_PY: &[u8] = include_bytes!("../resources/assets/reminder.py");
const REMINDER_TXT: &[u8] = include_bytes!("../resources/assets/reminder.txt");

pub fn sha256_reader(mut reader: impl Read) -> io::Result<String> {
    let mut digest = Sha256::new();
    io::copy(&mut reader, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

pub fn packaged_bundle() -> Result<Bundle, BundleError> {
    Bundle::from_memory(
        MANIFEST,
        [
            BundleEntry::directory("skills/checksum-review")?,
            BundleEntry::file("skills/checksum-review/SKILL.md", SKILL.to_vec())?,
            BundleEntry::directory("skills/checksum-review/assets")?,
            BundleEntry::file("skills/checksum-review/assets/sample.bin", SAMPLE.to_vec())?,
            BundleEntry::directory("skills/checksum-review/scripts")?,
            BundleEntry::executable_file(
                "skills/checksum-review/scripts/verify_checksum.py",
                VERIFY.to_vec(),
            )?,
            BundleEntry::directory("skills/checksum-review/scratch")?,
            BundleEntry::file(
                "skills/checksum-review/scratch/README.txt",
                SCRATCH.to_vec(),
            )?,
        ],
    )
}

pub fn configuration_bundle(
    guidance: &str,
    mcp_argv: &[String],
    mcp_url: &str,
) -> Result<Bundle, BundleError> {
    let skills = packaged_bundle()?;
    if skills.id() != BUNDLE_ID {
        return Err(BundleError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "packaged bundle ID differs from the host's stable ID",
        )));
    }
    let mut artifacts = skills.artifacts().to_vec();
    artifacts.push(Artifact::Instruction(InstructionArtifact::new(
        "guidance", guidance,
    )?));
    artifacts.push(Artifact::Mcp(McpArtifact::new(
        "local",
        "checksum-local",
        McpTransport::Command(Command::new(
            mcp_argv.iter().cloned(),
            [("CHECKSUM_MCP_TOKEN", EnvRef::new("CHECKSUM_MCP_TOKEN")?)],
            None,
        )?),
    )?));
    artifacts.push(Artifact::Mcp(McpArtifact::new(
        "remote",
        "checksum-remote",
        McpTransport::Http(HttpTransport::new(
            mcp_url,
            Some(EnvRef::new("CHECKSUM_HTTP_TOKEN")?),
        )?),
    )?));
    artifacts.push(Artifact::Hook(HookArtifact::new(
        "reminder",
        HookEvent::BeforeTool,
        Command::new(
            [
                flyrail::CommandArgument::from("python3"),
                flyrail::CommandArgument::from(AssetRef::with_path(
                    "reminder-assets",
                    "reminder.py",
                )?),
            ],
            Vec::<(String, String)>::new(),
            None,
        )?,
        10_000,
        [HookOutcome::Context],
        Vec::<String>::new(),
    )?));
    let assets = [SupportAsset::new(
        "reminder-assets",
        Family::Hooks,
        TreeContent::new([
            BundleEntry::file("reminder.py", REMINDER_PY.to_vec())?,
            BundleEntry::file("reminder.txt", REMINDER_TXT.to_vec())?,
        ])?,
    )?];
    Bundle::from_artifacts(
        BundleIdentity::new(CONFIG_BUNDLE_ID, skills.version())?,
        artifacts,
        assets,
        [Dependency::new("reminder", "reminder-assets")?],
    )
}

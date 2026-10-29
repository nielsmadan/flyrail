use flyrail::{Agent, Bundle, Target, inspect, install, render_skills};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let operation = arguments.next().ok_or("missing operation")?;
    let source = arguments.next().ok_or("missing source")?;
    let destination = arguments.next().ok_or("missing destination")?;
    let bundle = Bundle::from_directory(source)?;
    let target = Target::directory(&destination)?;
    let rendered = render_skills(&bundle, &target)?;
    let droid = render_skills(&bundle, &Target::project(Agent::Droid, &destination)?)?;
    let status = if operation == "install" {
        let results = install(&bundle, [target.clone()])?;
        format!(
            "{:?}",
            results.first().ok_or("missing result")?.status().as_str()
        )
    } else {
        "null".to_owned()
    };
    let inspections = inspect(&bundle, [target])?;
    let observed = inspections
        .first()
        .ok_or("missing inspection")?
        .observation();
    let ids: Vec<_> = rendered.artifacts().iter().map(|item| item.id()).collect();
    let droid_ids: Vec<_> = droid
        .notices()
        .iter()
        .map(|item| item.artifact_id())
        .collect();
    println!(
        "{{\"status\":{status},\"current\":{},\"content_matches\":{},\"ids\":{ids:?},\"render_digest\":{:?},\"droid_ids\":{droid_ids:?}}}",
        observed.is_current(),
        observed.content_matches().unwrap_or(false),
        rendered.content_digest()
    );
    Ok(())
}

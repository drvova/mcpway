use crate::support::stdio_child::CommandSpec;

pub fn parse_command_spec(cmd: &str) -> Result<CommandSpec, String> {
    let parts = shell_words::split(cmd).map_err(|err| err.to_string())?;
    if parts.is_empty() {
        return Err("stdio command is empty".into());
    }
    Ok(CommandSpec {
        program: parts[0].clone(),
        args: parts[1..].to_vec(),
    })
}

use crate::CommandSpec;

pub fn all_commands() -> Vec<CommandSpec> {
    let mut v = Vec::new();
    v.extend(crate::system_cmds::specs());
    v.extend(crate::capture_cmds::specs());
    v.extend(crate::doc_cmds::specs());
    v.extend(crate::editor_cmds::specs());
    v.extend(crate::image_cmds::specs());
    v.extend(crate::fx_cmds::specs());
    v.extend(crate::library_cmds::specs());
    v.extend(crate::record_cmds::specs());
    v.extend(crate::advanced_cmds::specs());
    v.extend(crate::video_cmds::specs());
    v.extend(crate::polish_cmds::specs());
    v.extend(crate::speech_cmds::specs());
    v.extend(crate::ml_cmds::specs());
    v.extend(crate::secrets::specs());
    v.extend(crate::tutor_cmds::specs());
    v.extend(crate::update_cmds::specs());
    v
}

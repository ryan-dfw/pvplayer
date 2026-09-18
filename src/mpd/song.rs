use anyhow::Context;

#[derive(Debug)]
pub struct Song {
    pub file: String,
    pub title: Option<String>
}

impl Song {
    pub (super) fn parse(lines: &[String]) -> anyhow::Result<Self> {
        let mut file = None;
        let mut title = None;

        for line in lines {
            let line = line.strip_suffix('\n').unwrap_or(line);

            if let Some(value) = line.strip_prefix("file: ") {
                file = Some(value.to_owned());
            }

            if let Some(value) = line.strip_prefix("Title: ") {
                title = Some(value.to_owned());
            }
        }

        Ok(Self {
            file: file.context("MPD song response is missing its file")?,
            title
        })
    }
}
use serde::{Deserialize, Serialize};

pub(crate) const MAX_IMAGES_PER_REQUEST: usize = 4;
pub(crate) const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Attachment {
    #[serde(rename = "mimeType")]
    pub(crate) mime_type: String,
    pub(crate) data: String,
}

impl Attachment {
    pub(crate) fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }
    pub(crate) fn as_data_uri(&self) -> String {
        format!("data:{};base64,{}", &self.mime_type, &self.data)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UserMediaData {
    pub(crate) images: Vec<String>,
}

impl UserMediaData {
    pub(crate) fn from_attachments(attachments: &[Attachment]) -> Result<Self, String> {
        let mut images = Vec::with_capacity(attachments.len());
        for a in attachments {
            if !a.is_image() {
                continue;
            }
            if a.data.len() > MAX_IMAGE_BYTES * 4 / 3 + 4 {
                return Err(String::from("Image is too large, max 10MB."));
            }
            if a.data.starts_with("http://") || a.data.starts_with("https://") {
                images.push(a.data.clone());
            } else {
                images.push(a.as_data_uri());
            }
        }
        if images.len() > MAX_IMAGES_PER_REQUEST {
            return Err(format!(
                "Too many images, max {MAX_IMAGES_PER_REQUEST}."
            ));
        }
        Ok(Self { images })
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

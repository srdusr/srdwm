pub type WorkspaceId = usize;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: String,
}

impl Workspace {
    pub fn new(id: WorkspaceId, name: impl Into<String>, layout: impl Into<String>) -> Self {
        Self { id, name: name.into(), layout: layout.into() }
    }
}

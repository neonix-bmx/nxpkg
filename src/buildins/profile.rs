#[derive(Debug, Default, Clone)]
pub struct BuildProfile {
    pub name: String,
    pub build_system: Option<String>,
    pub configure_args: Vec<String>,
    pub build_args: Vec<String>,
    pub install_args: Vec<String>,
}

impl BuildProfile {
    pub fn new(name: impl Into<String>) -> Self {
        BuildProfile {
            name: name.into(),
            build_system: None,
            configure_args: Vec::new(),
            build_args: Vec::new(),
            install_args: Vec::new(),
        }
    }
}

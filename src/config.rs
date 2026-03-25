pub struct EasyConnectConfig {
    pub server: String,
    pub username: String,
    pub password: String,
}

impl EasyConnectConfig {
    pub fn new(
        server: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            server: server.into(),
            username: username.into(),
            password: password.into(),
        }
    }
}

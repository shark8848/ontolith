#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TenantId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoleId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Permission {
    pub resource: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthContext {
    pub tenant: TenantId,
    pub user: UserId,
    pub roles: Vec<RoleId>,
    pub permissions: Vec<Permission>,
}

impl AuthContext {
    pub fn can(&self, resource: &str, action: &str) -> bool {
        self.permissions
            .iter()
            .any(|p| p.resource == resource && p.action == action)
    }
}

pub fn status() -> &'static str {
    "domain"
}

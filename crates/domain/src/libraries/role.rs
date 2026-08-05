#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RoleCode {
    Owner,
    Editor,
    Reader,
}

impl RoleCode {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "editor" => Some(Self::Editor),
            "reader" => Some(Self::Reader),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Editor => "editor",
            Self::Reader => "reader",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PermissionCode {
    LibraryManage,
    MemberInvite,
    HoldingView,
    HoldingEdit,
    ItemRead,
    ItemDownload,
}

impl PermissionCode {
    pub const ALL: [Self; 6] = [
        Self::LibraryManage,
        Self::MemberInvite,
        Self::HoldingView,
        Self::HoldingEdit,
        Self::ItemRead,
        Self::ItemDownload,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LibraryManage => "library.manage",
            Self::MemberInvite => "member.invite",
            Self::HoldingView => "holding.view",
            Self::HoldingEdit => "holding.edit",
            Self::ItemRead => "item.read",
            Self::ItemDownload => "item.download",
        }
    }
}

use ulid::Ulid;
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $name(Uuid);

        // An identifier has no meaningful default; `new` always mints a distinct value.
        #[allow(clippy::new_without_default)]
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }
    };
}

uuid_id!(UserId);
uuid_id!(SessionId);
uuid_id!(LibraryId);
uuid_id!(InvitationId);
uuid_id!(ManifestationId);
uuid_id!(ItemId);
uuid_id!(WorkId);
uuid_id!(ExpressionId);
uuid_id!(HoldingId);
uuid_id!(PublicationPackageId);
uuid_id!(ContentUnitId);
uuid_id!(BlobId);
uuid_id!(UploadId);
uuid_id!(JobId);
uuid_id!(DeviceId);

macro_rules! ulid_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $name(Ulid);

        // An identifier has no meaningful default; `new` always mints a distinct value.
        #[allow(clippy::new_without_default)]
        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            #[must_use]
            pub const fn from_ulid(value: Ulid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_ulid(self) -> Ulid {
                self.0
            }
        }
    };
}

ulid_id!(RequestId);
ulid_id!(ErrorId);

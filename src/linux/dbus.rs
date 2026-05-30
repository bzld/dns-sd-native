use zbus::proxy;

pub const _AVAHI_ENTRY_GROUP_UNCOMMITTED: i32 = 0;
pub const AVAHI_ENTRY_GROUP_REGISTERING: i32 = 1;
pub const AVAHI_ENTRY_GROUP_ESTABLISHED: i32 = 2;
pub const AVAHI_ENTRY_GROUP_COLLISION: i32 = 3;
pub const AVAHI_ENTRY_GROUP_FAILURE: i32 = 4;

#[proxy(
    interface = "org.freedesktop.Avahi.Server",
    default_service = "org.freedesktop.Avahi",
    default_path = "/"
)]
pub trait Avahi {
    #[zbus(object = "EntryGroup")]
    fn entry_group_new(&self);

    fn get_host_name(&self) -> zbus::Result<String>;
}

#[proxy(
    interface = "org.freedesktop.Avahi.EntryGroup",
    default_service = "org.freedesktop.Avahi"
)]
pub trait EntryGroup {
    #[allow(clippy::too_many_arguments)]
    fn add_service(
        &self,
        interface: i32,
        protocol: i32,
        flags: u32,
        name: &str,
        service_type: &str,
        domain: &str,
        host: &str,
        port: u16,
        txt: &[&[u8]],
    ) -> zbus::Result<()>;

    fn commit(&self) -> zbus::Result<()>;

    fn reset(&self) -> zbus::Result<()>;

    fn get_state(&self) -> zbus::Result<i32>;

    #[zbus(signal)]
    fn state_changed(&self, state: i32, error: &str) -> zbus::Result<()>;
}

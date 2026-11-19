//! Mesh (devices, network, pairing, replicas) UI and errors.

/// Static strings: mesh (devices, network, pairing, replicas) UI and errors.
pub struct MeshStrings {}

pub static RU: MeshStrings = MeshStrings {};

pub static EN: MeshStrings = MeshStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    mesh_in -> MeshStrings,
    /// Returns these strings for the active process locale.
    mesh,
    { Ru => RU, En => EN }
}

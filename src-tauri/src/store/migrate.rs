use super::{ConfigFile, ServicesFile, StoreError};

pub const SCHEMA_VERSION: u32 = 1;

pub fn ensure_supported(version: u32) -> Result<(), StoreError> {
    if version > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew { found: version });
    }
    Ok(())
}

/// v1 is a no-op. Older files are bumped to v1 with no field rewrite.
pub fn migrate_config(file: &mut ConfigFile) -> Result<bool, StoreError> {
    ensure_supported(file.schema_version)?;
    if file.schema_version == SCHEMA_VERSION {
        return Ok(false);
    }
    file.schema_version = SCHEMA_VERSION;
    Ok(true)
}

pub fn migrate_services(file: &mut ServicesFile) -> Result<bool, StoreError> {
    ensure_supported(file.schema_version)?;
    if file.schema_version == SCHEMA_VERSION {
        return Ok(false);
    }
    file.schema_version = SCHEMA_VERSION;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{ensure_supported, migrate_config, SCHEMA_VERSION};
    use crate::store::{ConfigFile, StoreError};

    #[test]
    fn v1_is_noop() {
        let mut file = ConfigFile::default();
        assert_eq!(file.schema_version, SCHEMA_VERSION);
        assert!(!migrate_config(&mut file).unwrap());
        assert_eq!(file.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn older_schema_bumps_to_v1() {
        let mut file = ConfigFile {
            schema_version: 0,
            ..ConfigFile::default()
        };
        assert!(migrate_config(&mut file).unwrap());
        assert_eq!(file.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn newer_schema_is_refused() {
        let err = ensure_supported(SCHEMA_VERSION + 1).unwrap_err();
        assert!(matches!(err, StoreError::SchemaTooNew { found: 2 }));
        assert_eq!(
            err.to_string(),
            "Pulse needs to be updated to read this config."
        );
    }
}

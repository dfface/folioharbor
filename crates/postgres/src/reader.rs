use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use folioharbor_application::ports::{
    ReadingRepository, ReadingRepositoryError, UpdateProgressRecord,
};
use folioharbor_domain::{
    id::{ContentUnitId, DeviceId, ManifestationId, PublicationPackageId, RequestId, UserId},
    reader::{
        DeviceReadingState, LocatorExtensionValue, LocatorExtensions, LocatorLocations,
        LocatorText, ReadingProgress, ReadingUpdateOutcome, ReadiumLocator,
    },
    time::OffsetDateTime,
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{DatabaseContext, PgTransactionContext};

const MUTATION_RETENTION_DAYS: i64 = 30;
const MUTATION_MAX_ROWS_PER_USER: i64 = 10_000;
const MUTATION_MAX_BYTES_PER_USER: i64 = 64 * 1_024 * 1_024;

#[derive(Clone)]
pub struct PgReadingRepository {
    pool: PgPool,
}
impl PgReadingRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ReadingRepository for PgReadingRepository {
    async fn get_progress(
        &self,
        actor: UserId,
        manifestation_id: ManifestationId,
        request_id: RequestId,
    ) -> Result<Option<ReadingProgress>, ReadingRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api_without_library(actor, request_id),
        )
        .await
        .map_err(persistence)?;
        require_readable(&mut tx, actor, manifestation_id).await?;
        let row = sqlx::query("SELECT package_id,content_unit_id,locator,version,updated_at FROM folioharbor.reading_states WHERE user_id=$1 AND manifestation_id=$2")
            .bind(actor.as_uuid()).bind(manifestation_id.as_uuid()).fetch_optional(&mut *tx).await.map_err(persistence)?;
        let progress = row
            .map(|row| progress_from_row(manifestation_id, &row))
            .transpose()?;
        tx.commit().await.map_err(persistence)?;
        Ok(progress)
    }

    async fn update_progress(
        &self,
        command: UpdateProgressRecord,
    ) -> Result<ReadingUpdateOutcome, ReadingRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence)?;
        PgTransactionContext::apply(
            &mut tx,
            &DatabaseContext::api_without_library(command.actor, command.request_id),
        )
        .await
        .map_err(persistence)?;
        require_readable(&mut tx, command.actor, command.manifestation_id).await?;
        lock_progress_update(&mut tx, &command).await?;

        let now: OffsetDateTime = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await
            .map_err(persistence)?;
        ensure_active_device(&mut tx, command.actor, command.device_id, now).await?;
        prune_expired_mutations(&mut tx, command.actor, now).await?;
        let fingerprint = command_fingerprint(&command);
        if let Some(outcome) = replay(&mut tx, &command, &fingerprint).await? {
            tx.commit().await.map_err(persistence)?;
            return Ok(outcome);
        }
        ensure_row_capacity(&mut tx, command.actor, now).await?;

        let existing = sqlx::query("SELECT package_id,content_unit_id,locator,version,updated_at FROM folioharbor.reading_states WHERE user_id=$1 AND manifestation_id=$2 FOR UPDATE")
            .bind(command.actor.as_uuid()).bind(command.manifestation_id.as_uuid()).fetch_optional(&mut *tx).await.map_err(persistence)?;
        let locator_json = locator_to_json(&command.locator);
        sqlx::query("INSERT INTO folioharbor.device_reading_states(user_id,device_id,manifestation_id,locator,updated_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id,device_id,manifestation_id) DO UPDATE SET locator=EXCLUDED.locator,updated_at=EXCLUDED.updated_at")
            .bind(command.actor.as_uuid()).bind(command.device_id.as_uuid()).bind(command.manifestation_id.as_uuid()).bind(&locator_json).bind(now).execute(&mut *tx).await.map_err(persistence)?;
        let device = DeviceReadingState {
            device_id: command.device_id,
            locator: command.locator.clone(),
            updated_at: now,
        };

        let (global, updated) = if let Some(row) = existing {
            let current = progress_from_row(command.manifestation_id, &row)?;
            if current.version == command.base_version {
                let next_version = current
                    .version
                    .checked_add(1)
                    .ok_or(ReadingRepositoryError::Persistence)?;
                sqlx::query("UPDATE folioharbor.reading_states SET package_id=$3,content_unit_id=$4,locator=$5,version=$6,updated_at=$7 WHERE user_id=$1 AND manifestation_id=$2")
                    .bind(command.actor.as_uuid()).bind(command.manifestation_id.as_uuid()).bind(command.package_id.map(PublicationPackageId::as_uuid)).bind(command.content_unit_id.map(ContentUnitId::as_uuid)).bind(&locator_json).bind(i64::try_from(next_version).map_err(|_| ReadingRepositoryError::Persistence)?).bind(now).execute(&mut *tx).await.map_err(persistence)?;
                (
                    Some(ReadingProgress {
                        manifestation_id: command.manifestation_id,
                        package_id: command.package_id,
                        content_unit_id: command.content_unit_id,
                        locator: command.locator.clone(),
                        version: next_version,
                        updated_at: now,
                    }),
                    true,
                )
            } else {
                (Some(current), false)
            }
        } else if command.base_version == 0 {
            sqlx::query("INSERT INTO folioharbor.reading_states(user_id,manifestation_id,package_id,content_unit_id,locator,version,updated_at) VALUES($1,$2,$3,$4,$5,1,$6)")
                .bind(command.actor.as_uuid()).bind(command.manifestation_id.as_uuid()).bind(command.package_id.map(PublicationPackageId::as_uuid)).bind(command.content_unit_id.map(ContentUnitId::as_uuid)).bind(&locator_json).bind(now).execute(&mut *tx).await.map_err(persistence)?;
            (
                Some(ReadingProgress {
                    manifestation_id: command.manifestation_id,
                    package_id: command.package_id,
                    content_unit_id: command.content_unit_id,
                    locator: command.locator.clone(),
                    version: 1,
                    updated_at: now,
                }),
                true,
            )
        } else {
            (None, false)
        };
        let outcome = update_outcome(global.as_ref(), &device, updated);
        let retry_after = capacity_retry_after(&mut tx, command.actor, now).await?;
        if let Err(error) = store_mutation(
            &mut tx,
            &command,
            &fingerprint,
            global.as_ref(),
            &device,
            updated,
            retry_after,
        )
        .await
        {
            tx.rollback().await.map_err(persistence)?;
            return Err(error);
        }
        tx.commit().await.map_err(persistence)?;
        Ok(outcome)
    }
}

fn update_outcome(
    global: Option<&ReadingProgress>,
    device: &DeviceReadingState,
    updated: bool,
) -> ReadingUpdateOutcome {
    if updated {
        match global {
            Some(global) => ReadingUpdateOutcome::Updated {
                global: global.clone(),
                device: device.clone(),
            },
            None => ReadingUpdateOutcome::Conflict {
                global: None,
                device: device.clone(),
            },
        }
    } else {
        ReadingUpdateOutcome::Conflict {
            global: global.cloned(),
            device: device.clone(),
        }
    }
}

async fn ensure_active_device(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: UserId,
    device: DeviceId,
    now: OffsetDateTime,
) -> Result<(), ReadingRepositoryError> {
    sqlx::query("INSERT INTO folioharbor.user_devices(device_id,user_id,display_name,created_at,last_seen_at) VALUES($1,$2,'Web reader',$3,$3) ON CONFLICT DO NOTHING")
        .bind(device.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&mut **tx).await.map_err(persistence)?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM folioharbor.user_devices WHERE user_id=$1 AND device_id=$2 AND revoked_at IS NULL)")
        .bind(actor.as_uuid()).bind(device.as_uuid()).fetch_one(&mut **tx).await.map_err(persistence)?;
    if exists {
        sqlx::query("UPDATE folioharbor.user_devices SET last_seen_at=$3,version=version+1 WHERE user_id=$1 AND device_id=$2 AND revoked_at IS NULL")
            .bind(actor.as_uuid()).bind(device.as_uuid()).bind(now).execute(&mut **tx).await.map_err(persistence)?;
        Ok(())
    } else {
        Err(ReadingRepositoryError::NotFound)
    }
}

async fn store_mutation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &UpdateProgressRecord,
    fingerprint: &[u8; 32],
    global: Option<&ReadingProgress>,
    device: &DeviceReadingState,
    updated: bool,
    retry_after: Duration,
) -> Result<(), ReadingRepositoryError> {
    let version = global.map_or(Ok(0), |state| {
        i64::try_from(state.version).map_err(|_| ReadingRepositoryError::Persistence)
    })?;
    sqlx::query("INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,request_fingerprint) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)")
        .bind(command.actor.as_uuid()).bind(command.client_mutation_id).bind(command.manifestation_id.as_uuid()).bind(command.device_id.as_uuid()).bind(if updated { "updated" } else { "conflict" }).bind(global.and_then(|state|state.package_id.map(PublicationPackageId::as_uuid))).bind(global.and_then(|state|state.content_unit_id.map(ContentUnitId::as_uuid))).bind(global.map(|state|locator_to_json(&state.locator))).bind(version).bind(global.map(|state|state.updated_at)).bind(locator_to_json(&device.locator)).bind(device.updated_at).bind(fingerprint.as_slice()).execute(&mut **tx).await.map_err(|error| mutation_persistence(&error, retry_after))?;
    Ok(())
}

fn mutation_persistence(error: &sqlx::Error, retry_after: Duration) -> ReadingRepositoryError {
    if error
        .as_database_error()
        .and_then(|error| error.constraint())
        == Some("reading_mutation_quota")
    {
        ReadingRepositoryError::MutationCapacity { retry_after }
    } else {
        ReadingRepositoryError::Persistence
    }
}

async fn prune_expired_mutations(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: UserId,
    now: OffsetDateTime,
) -> Result<(), ReadingRepositoryError> {
    let cutoff = now - time::Duration::days(MUTATION_RETENTION_DAYS);
    sqlx::query("DELETE FROM folioharbor.reading_mutations WHERE user_id=$1 AND created_at<$2")
        .bind(actor.as_uuid())
        .bind(cutoff)
        .execute(&mut **tx)
        .await
        .map_err(persistence)?;
    Ok(())
}

async fn ensure_row_capacity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: UserId,
    now: OffsetDateTime,
) -> Result<(), ReadingRepositoryError> {
    let usage: Option<(i64, i64)> = sqlx::query_as(
        "SELECT live_count,live_bytes FROM folioharbor.reading_mutation_usage WHERE user_id=$1",
    )
    .bind(actor.as_uuid())
    .fetch_optional(&mut **tx)
    .await
    .map_err(persistence)?;
    if usage.is_some_and(|usage| {
        usage.0 >= MUTATION_MAX_ROWS_PER_USER || usage.1 >= MUTATION_MAX_BYTES_PER_USER
    }) {
        Err(capacity_error(tx, actor, now).await?)
    } else {
        Ok(())
    }
}

async fn capacity_error(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: UserId,
    now: OffsetDateTime,
) -> Result<ReadingRepositoryError, ReadingRepositoryError> {
    Ok(ReadingRepositoryError::MutationCapacity {
        retry_after: capacity_retry_after(tx, actor, now).await?,
    })
}

async fn capacity_retry_after(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: UserId,
    now: OffsetDateTime,
) -> Result<Duration, ReadingRepositoryError> {
    let oldest: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT min(created_at) FROM folioharbor.reading_mutations WHERE user_id=$1",
    )
    .bind(actor.as_uuid())
    .fetch_one(&mut **tx)
    .await
    .map_err(persistence)?;
    Ok(oldest.map_or(Duration::from_secs(1), |created_at| {
        retry_after_duration(
            created_at + time::Duration::days(MUTATION_RETENTION_DAYS),
            now,
        )
    }))
}

fn retry_after_duration(expires_at: OffsetDateTime, now: OffsetDateTime) -> Duration {
    const NANOS_PER_SECOND: i128 = 1_000_000_000;
    let remaining_nanos = (expires_at - now).whole_nanoseconds();
    let rounded_seconds = ((remaining_nanos + NANOS_PER_SECOND - 1) / NANOS_PER_SECOND).max(1);
    Duration::from_secs(u64::try_from(rounded_seconds).unwrap_or(u64::MAX))
}

async fn require_readable(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: UserId,
    manifestation: ManifestationId,
) -> Result<(), ReadingRepositoryError> {
    let readable: bool =
        sqlx::query_scalar("SELECT folioharbor.progress_manifestation_readable($1,$2)")
            .bind(actor.as_uuid())
            .bind(manifestation.as_uuid())
            .fetch_one(&mut **tx)
            .await
            .map_err(persistence)?;
    if readable {
        Ok(())
    } else {
        Err(ReadingRepositoryError::NotFound)
    }
}
async fn advisory_lock(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    key: String,
) -> Result<(), ReadingRepositoryError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(key)
        .execute(&mut **tx)
        .await
        .map_err(persistence)?;
    Ok(())
}
async fn lock_progress_update(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &UpdateProgressRecord,
) -> Result<(), ReadingRepositoryError> {
    for key in [
        format!("progress:mutation-capacity:{}", command.actor.as_uuid()),
        format!(
            "progress:mutation:{}:{}",
            command.actor.as_uuid(),
            command.client_mutation_id
        ),
        format!(
            "progress:state:{}:{}",
            command.actor.as_uuid(),
            command.manifestation_id.as_uuid()
        ),
    ] {
        advisory_lock(tx, key).await?;
    }
    Ok(())
}
async fn replay(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    command: &UpdateProgressRecord,
    fingerprint: &[u8; 32],
) -> Result<Option<ReadingUpdateOutcome>, ReadingRepositoryError> {
    let row = sqlx::query("SELECT manifestation_id,device_id,request_fingerprint,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at FROM folioharbor.reading_mutations WHERE user_id=$1 AND client_mutation_id=$2")
        .bind(command.actor.as_uuid()).bind(command.client_mutation_id).fetch_optional(&mut **tx).await.map_err(persistence)?;
    row.map(|row| {
        let stored_manifestation: Uuid = row.try_get("manifestation_id").map_err(persistence)?;
        let stored_device: Uuid = row.try_get("device_id").map_err(persistence)?;
        let stored_fingerprint: Option<Vec<u8>> =
            row.try_get("request_fingerprint").map_err(persistence)?;
        if stored_manifestation != command.manifestation_id.as_uuid()
            || stored_device != command.device_id.as_uuid()
            || stored_fingerprint.as_deref() != Some(fingerprint.as_slice())
        {
            return Err(ReadingRepositoryError::MutationMismatch);
        }
        let version = row
            .try_get::<i64, _>("global_version")
            .map_err(persistence)?;
        let global = if version == 0 {
            let locator: Option<Value> = row.try_get("global_locator").map_err(persistence)?;
            let updated_at: Option<OffsetDateTime> =
                row.try_get("global_updated_at").map_err(persistence)?;
            if locator.is_some() || updated_at.is_some() {
                return Err(ReadingRepositoryError::Persistence);
            }
            None
        } else {
            Some(ReadingProgress {
                manifestation_id: command.manifestation_id,
                package_id: optional_id(
                    &row,
                    "global_package_id",
                    PublicationPackageId::from_uuid,
                )?,
                content_unit_id: optional_id(
                    &row,
                    "global_content_unit_id",
                    ContentUnitId::from_uuid,
                )?,
                locator: locator_from_json(&row.try_get("global_locator").map_err(persistence)?)?,
                version: u64::try_from(version).map_err(|_| ReadingRepositoryError::Persistence)?,
                updated_at: row.try_get("global_updated_at").map_err(persistence)?,
            })
        };
        let device = DeviceReadingState {
            device_id: DeviceId::from_uuid(row.try_get("device_id").map_err(persistence)?),
            locator: locator_from_json(&row.try_get("device_locator").map_err(persistence)?)?,
            updated_at: row.try_get("device_updated_at").map_err(persistence)?,
        };
        let kind: String = row.try_get("outcome").map_err(persistence)?;
        match kind.as_str() {
            "updated" => global.map_or(Err(ReadingRepositoryError::Persistence), |global| {
                Ok(ReadingUpdateOutcome::Updated { global, device })
            }),
            "conflict" => Ok(ReadingUpdateOutcome::Conflict { global, device }),
            _ => Err(ReadingRepositoryError::Persistence),
        }
    })
    .transpose()
}

fn command_fingerprint(command: &UpdateProgressRecord) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"folioharbor-progress-command-v1\0");
    hash.update(command.manifestation_id.as_uuid().as_bytes());
    hash.update(command.device_id.as_uuid().as_bytes());
    hash.update(command.base_version.to_be_bytes());
    update_optional_uuid(
        &mut hash,
        command.package_id.map(PublicationPackageId::as_uuid),
    );
    update_optional_uuid(
        &mut hash,
        command.content_unit_id.map(ContentUnitId::as_uuid),
    );
    hash.update(locator_to_json(&command.locator).to_string().as_bytes());
    hash.finalize().into()
}

fn update_optional_uuid(hash: &mut Sha256, value: Option<Uuid>) {
    match value {
        Some(value) => {
            hash.update([1]);
            hash.update(value.as_bytes());
        }
        None => hash.update([0]),
    }
}

fn progress_from_row(
    manifestation_id: ManifestationId,
    row: &sqlx::postgres::PgRow,
) -> Result<ReadingProgress, ReadingRepositoryError> {
    Ok(ReadingProgress {
        manifestation_id,
        package_id: optional_id(row, "package_id", PublicationPackageId::from_uuid)?,
        content_unit_id: optional_id(row, "content_unit_id", ContentUnitId::from_uuid)?,
        locator: locator_from_json(&row.try_get("locator").map_err(persistence)?)?,
        version: u64::try_from(row.try_get::<i64, _>("version").map_err(persistence)?)
            .map_err(|_| ReadingRepositoryError::Persistence)?,
        updated_at: row.try_get("updated_at").map_err(persistence)?,
    })
}
fn optional_id<T>(
    row: &sqlx::postgres::PgRow,
    name: &str,
    map: impl FnOnce(Uuid) -> T,
) -> Result<Option<T>, ReadingRepositoryError> {
    Ok(row
        .try_get::<Option<Uuid>, _>(name)
        .map_err(persistence)?
        .map(map))
}
fn persistence(_: sqlx::Error) -> ReadingRepositoryError {
    ReadingRepositoryError::Persistence
}

fn locator_to_json(locator: &ReadiumLocator) -> Value {
    let mut extensions = Map::new();
    for (key, value) in locator.extensions().values() {
        let value = match value {
            LocatorExtensionValue::Boolean(v) => json!(v),
            LocatorExtensionValue::Integer(v) => json!(v),
            LocatorExtensionValue::Number(v) => json!(v),
            LocatorExtensionValue::String(v) => json!(v),
        };
        extensions.insert(key.clone(), value);
    }
    json!({"href":locator.href(),"mediaType":locator.media_type(),"locations":{"progression":locator.locations().progression(),"position":locator.locations().position(),"totalProgression":locator.locations().total_progression(),"fragments":locator.locations().fragments()},"text":locator.text().map(|text| json!({"before":text.before(),"highlight":text.highlight(),"after":text.after()})),"extensions":{"version":locator.extensions().version(),"values":extensions}})
}
fn locator_from_json(value: &Value) -> Result<ReadiumLocator, ReadingRepositoryError> {
    let object = value
        .as_object()
        .ok_or(ReadingRepositoryError::Persistence)?;
    let locations = object
        .get("locations")
        .and_then(Value::as_object)
        .ok_or(ReadingRepositoryError::Persistence)?;
    let fragments = locations
        .get("fragments")
        .and_then(Value::as_array)
        .ok_or(ReadingRepositoryError::Persistence)?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or(ReadingRepositoryError::Persistence)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let text = object
        .get("text")
        .filter(|v| !v.is_null())
        .map(|value| {
            let value = value
                .as_object()
                .ok_or(ReadingRepositoryError::Persistence)?;
            LocatorText::new(
                optional_string(value, "before")?,
                optional_string(value, "highlight")?,
                optional_string(value, "after")?,
            )
            .map_err(|_| ReadingRepositoryError::Persistence)
        })
        .transpose()?;
    let extension_object = object
        .get("extensions")
        .and_then(Value::as_object)
        .ok_or(ReadingRepositoryError::Persistence)?;
    let version = extension_object
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|v| u16::try_from(v).ok())
        .ok_or(ReadingRepositoryError::Persistence)?;
    let mut extensions = BTreeMap::new();
    for (key, value) in extension_object
        .get("values")
        .and_then(Value::as_object)
        .ok_or(ReadingRepositoryError::Persistence)?
    {
        let value = if let Some(v) = value.as_bool() {
            LocatorExtensionValue::Boolean(v)
        } else if let Some(v) = value.as_i64() {
            LocatorExtensionValue::Integer(v)
        } else if let Some(v) = value.as_f64() {
            LocatorExtensionValue::Number(v)
        } else if let Some(v) = value.as_str() {
            LocatorExtensionValue::String(v.to_owned())
        } else {
            return Err(ReadingRepositoryError::Persistence);
        };
        extensions.insert(key.clone(), value);
    }
    ReadiumLocator::new(
        object
            .get("href")
            .and_then(Value::as_str)
            .ok_or(ReadingRepositoryError::Persistence)?
            .to_owned(),
        object
            .get("mediaType")
            .filter(|v| !v.is_null())
            .and_then(Value::as_str)
            .map(str::to_owned),
        LocatorLocations::new(
            optional_f64(locations, "progression")?,
            optional_u32(locations, "position")?,
            optional_f64(locations, "totalProgression")?,
            fragments,
        )
        .map_err(|_| ReadingRepositoryError::Persistence)?,
        text,
        LocatorExtensions::new(version, extensions)
            .map_err(|_| ReadingRepositoryError::Persistence)?,
    )
    .map_err(|_| ReadingRepositoryError::Persistence)
}
fn optional_string(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ReadingRepositoryError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(v)) => Ok(Some(v.clone())),
        _ => Err(ReadingRepositoryError::Persistence),
    }
}
fn optional_f64(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, ReadingRepositoryError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(Some)
            .ok_or(ReadingRepositoryError::Persistence),
    }
}
fn optional_u32(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<u32>, ReadingRepositoryError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or(ReadingRepositoryError::Persistence),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_rounds_up_fractional_seconds() {
        let now = OffsetDateTime::UNIX_EPOCH;
        let expires_at = now + time::Duration::milliseconds(1_001);

        assert_eq!(
            retry_after_duration(expires_at, now),
            Duration::from_secs(2)
        );
    }
}

CREATE INDEX audit_events_download_denial_aggregation_idx
ON folioharbor.audit_events(actor_id,resource_id,occurred_at DESC)
WHERE action_code='item.download' AND decision='denied';

UPDATE folioharbor.schema_metadata
SET schema_version=23,applied_at=clock_timestamp()
WHERE singleton;

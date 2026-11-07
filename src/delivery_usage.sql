WITH incoming AS (
 SELECT i.* FROM jsonb_populate_recordset(NULL::tenant_service_samples,$1) i
 WHERE NOT EXISTS (SELECT 1 FROM tenant_service_samples p
   WHERE p.tenant_id=i.tenant_id AND p.sampled_at>=i.sampled_at)
),
previous AS (
 SELECT i.tenant_id,p.sampled_at previous_at,p.epoch previous_epoch,p.deliveries previous_deliveries
 FROM incoming i JOIN LATERAL (SELECT * FROM tenant_service_samples p
 WHERE p.tenant_id=i.tenant_id AND p.sampled_at<i.sampled_at ORDER BY sampled_at DESC LIMIT 1) p ON true
), inserted AS (
 INSERT INTO tenant_service_samples SELECT * FROM incoming ON CONFLICT DO NOTHING RETURNING *
), deltas AS (
 SELECT i.tenant_id,(to_timestamp(i.sampled_at/1000.0) AT TIME ZONE 'UTC')::date AS day,
 i.deliveries-p.previous_deliveries deliveries,i.sampled_at-p.previous_at observed_ms
 FROM inserted i JOIN previous p USING(tenant_id)
 WHERE i.epoch IS NOT NULL AND i.epoch=p.previous_epoch
 AND i.deliveries>=p.previous_deliveries AND i.sampled_at-p.previous_at BETWEEN 1 AND 30000
 -- Do not attribute an interval straddling midnight to either day.
 AND (to_timestamp(i.sampled_at/1000.0) AT TIME ZONE 'UTC')::date =
     (to_timestamp(p.previous_at/1000.0) AT TIME ZONE 'UTC')::date
) INSERT INTO tenant_delivery_daily SELECT * FROM deltas
ON CONFLICT (tenant_id,day) DO UPDATE SET
 deliveries=tenant_delivery_daily.deliveries+EXCLUDED.deliveries,
 observed_ms=tenant_delivery_daily.observed_ms+EXCLUDED.observed_ms

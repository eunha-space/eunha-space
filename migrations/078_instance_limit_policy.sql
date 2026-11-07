-- A quota change can wake reconciliation without shortening an in-flight lease.
ALTER TABLE runtimes ADD COLUMN reconcile_requested boolean NOT NULL DEFAULT false,
 ADD COLUMN reconcile_lease_until timestamptz;
UPDATE runtimes SET reconcile_lease_until=reconcile_next_at WHERE reconcile_next_at>now()+interval '1 minute';

-- Roll out member visibility first. Warning and enforcement require an audited
-- operator choice after reviewing the collection window.
ALTER TABLE billing_settings ADD COLUMN usage_mode text NOT NULL DEFAULT 'show'
 CHECK (usage_mode IN ('show','warn','enforce'));
ALTER TABLE tenants ADD COLUMN limit_registrations boolean NOT NULL DEFAULT true,
 ADD COLUMN limit_media_uploads boolean NOT NULL DEFAULT true,
 ADD COLUMN limits_valid_until bigint;
ALTER TABLE billing_emails DROP CONSTRAINT billing_emails_kind_check;
ALTER TABLE billing_emails ADD CONSTRAINT billing_emails_kind_check
 CHECK (kind IN ('receipt','renewal_notice','refund','usage_notice'));
CREATE TABLE tenant_limit_states (
 tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
 resource text NOT NULL CHECK(resource IN ('users','media','database','activities')),
 plan_id uuid REFERENCES plans(id),
 over_since timestamptz, grace_until timestamptz, notified_at timestamptz,
 enforced_at timestamptz, warn_period text, exempt boolean NOT NULL DEFAULT false,
 PRIMARY KEY(tenant_id,resource)
);
CREATE TABLE tenant_limit_events (
 id bigserial PRIMARY KEY, tenant_id uuid REFERENCES tenants(id) ON DELETE SET NULL,
 resource text, actor_id uuid REFERENCES users(id), action text NOT NULL,
 reason text, detail jsonb NOT NULL DEFAULT '{}', created_at timestamptz NOT NULL DEFAULT now()
);
CREATE VIEW tenant_limit_readings AS
 SELECT t.id tenant_id,t.user_id,t.domain,t.lifecycle_state,p.id plan_id,
 COALESCE(s.current_period_end::text,date_trunc('month',now())::text) period,
 u.sampled_at, x.resource,x.value,x.allowance,
 (a.is_admin OR a.billing_exempt OR COALESCE(st.exempt,false)
   OR (x.resource='media' AND EXISTS(SELECT 1 FROM tenant_imports i WHERE i.tenant_id=t.id AND i.external_media))) exempt,
 (u.sampled_at BETWEEN extract(epoch FROM now()-interval '30 minutes')*1000 AND extract(epoch FROM now())*1000+30000 AND x.value IS NOT NULL) fresh
 FROM tenants t JOIN users a ON a.id=t.user_id
 LEFT JOIN subscriptions s ON s.user_id=t.user_id AND s.status IN ('active','past_due')
 LEFT JOIN plans p ON p.id=s.plan_id
 LEFT JOIN LATERAL (SELECT * FROM tenant_usage WHERE tenant_id=t.id ORDER BY sampled_at DESC LIMIT 1) u ON true
 CROSS JOIN LATERAL (VALUES
 ('users',u.users::numeric,p.user_limit::numeric),
 ('media',u.local_media_bytes::numeric,p.media_gb::numeric*1000000000),
 ('database',u.database_bytes::numeric,p.database_gb::numeric*1000000000),
 ('activities',(SELECT sum(deliveries)::numeric FROM tenant_delivery_daily d WHERE d.tenant_id=t.id AND d.day>=date_trunc('month',now() AT TIME ZONE 'UTC')::date),p.monthly_activities::numeric)
 ) x(resource,value,allowance)
 LEFT JOIN tenant_limit_states st ON st.tenant_id=t.id AND st.resource=x.resource;

-- All policy arithmetic lives here, including plan changes/refunds, samples,
-- exemptions and daily checks. Runtime flags are leased, fail-open decisions.
CREATE FUNCTION refresh_tenant_limits(target uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE r record; previous tenant_limit_states; current_state tenant_limit_states;
 mode text; notice text; notice_key text; block_users boolean:=false; block_media boolean:=false;
 valid_until bigint; lang text; changed boolean; resource_label text; usage_text text; allowance_text text;
BEGIN
 PERFORM 1 FROM tenants WHERE id=target FOR UPDATE;
 SELECT usage_mode INTO mode FROM billing_settings;
 SELECT COALESCE(locale,'en') INTO lang FROM users WHERE id=(SELECT user_id FROM tenants WHERE id=target);
 FOR r IN SELECT * FROM tenant_limit_readings WHERE tenant_id=target LOOP
  INSERT INTO tenant_limit_states(tenant_id,resource) VALUES(target,r.resource) ON CONFLICT DO NOTHING;
  SELECT * INTO previous FROM tenant_limit_states WHERE tenant_id=target AND resource=r.resource FOR UPDATE;
  current_state:=previous; notice:=NULL; notice_key:=NULL;
  resource_label:=CASE WHEN lang='ko' THEN CASE r.resource WHEN 'users' THEN '사용자' WHEN 'media' THEN '로컬 미디어' WHEN 'database' THEN '데이터베이스' ELSE '월간 외부 전송 (UTC)' END ELSE CASE r.resource WHEN 'users' THEN 'Users' WHEN 'media' THEN 'Local media' WHEN 'database' THEN 'Database' ELSE 'Outbound deliveries this month (UTC)' END END;
  usage_text:=CASE WHEN r.resource IN ('media','database') THEN round(r.value/1000000000,2)::text||' GB' ELSE r.value::text END;
  allowance_text:=CASE WHEN r.resource IN ('media','database') THEN round(r.allowance/1000000000,2)::text||' GB' ELSE r.allowance::text END;
  current_state.plan_id:=r.plan_id;
  IF mode='show' OR r.exempt OR r.plan_id IS NULL OR r.lifecycle_state NOT IN ('ready','failed') OR NOT COALESCE(r.fresh,false) OR r.allowance IS NULL THEN
   current_state.over_since:=NULL; current_state.grace_until:=NULL; current_state.enforced_at:=NULL;
  ELSIF r.value < r.allowance THEN
   IF previous.over_since IS NOT NULL THEN
    notice:='recovered'; notice_key:='recovered:'||previous.over_since::text;
   END IF;
   current_state.over_since:=NULL; current_state.grace_until:=NULL; current_state.enforced_at:=NULL;
   IF notice IS NULL AND r.value >= r.allowance*0.8 AND previous.warn_period IS DISTINCT FROM r.period THEN
    notice:='near'; notice_key:='near:'||r.period; current_state.warn_period:=r.period;
   END IF;
  ELSE
   IF previous.over_since IS NULL OR previous.plan_id IS DISTINCT FROM r.plan_id THEN
    current_state.over_since:=now();
    current_state.grace_until:=CASE WHEN r.resource IN ('users','media') THEN now()+interval '14 days' END;
    current_state.enforced_at:=NULL;
    current_state.warn_period:=r.period;
    notice:='over'; notice_key:='over:'||current_state.over_since::text;
   END IF;
   IF mode='enforce' AND r.resource IN ('users','media') AND current_state.grace_until<=now() THEN
    current_state.enforced_at:=COALESCE(current_state.enforced_at,now());
   ELSE current_state.enforced_at:=NULL;
   END IF;
  END IF;
  -- A delayed near/over message must not outlive its decision or exemption.
  IF notice IS NOT NULL OR mode='show' OR r.exempt OR r.plan_id IS NULL OR NOT COALESCE(r.fresh,false) OR r.allowance IS NULL OR r.value<r.allowance*0.8 THEN
   UPDATE billing_emails SET abandoned_at=now() WHERE kind='usage_notice' AND sent_at IS NULL AND abandoned_at IS NULL
    AND dedupe_key LIKE 'usage:'||target||':'||r.resource||':%';
  END IF;
  IF notice IS NOT NULL THEN
   INSERT INTO billing_emails(user_id,kind,dedupe_key,locale,subject,body)
   VALUES(r.user_id,'usage_notice','usage:'||target||':'||r.resource||':'||notice_key,lang,
    CASE WHEN lang='ko' THEN '은하공간 인스턴스 사용량 안내' ELSE 'Your eunha space instance usage' END,
    CASE WHEN lang='ko' THEN
     format('%s · %s: %s / %s. %s 계정 페이지에서 사용량과 측정 시각을 확인하세요.',r.domain,resource_label,usage_text,allowance_text,
      CASE notice WHEN 'recovered' THEN '사용량이 허용량 아래로 내려왔습니다.' WHEN 'near' THEN '요금제 허용량의 80%에 도달했습니다.' ELSE
       CASE WHEN r.resource IN ('users','media') THEN '허용량에 도달했습니다. 유예 기간은 14일이며, 제한 적용 시 새 가입 또는 미디어 업로드만 중단됩니다. 기존 서비스는 계속 운영됩니다.' ELSE '허용량에 도달했습니다. 공정 이용 안내이며 자동으로 서비스를 제한하지 않습니다.' END END)
    ELSE format('%s · %s: %s / %s. %s See your account for readings and measurement times.',r.domain,resource_label,usage_text,allowance_text,
      CASE notice WHEN 'recovered' THEN 'Usage is below the allowance again.' WHEN 'near' THEN 'Usage has reached 80% of the plan allowance.' ELSE
       CASE WHEN r.resource IN ('users','media') THEN 'Usage has reached the allowance. The grace period is 14 days. Enforcement only closes new registrations or media uploads; the instance keeps serving its members.' ELSE 'Usage has reached the fair-use allowance. No automatic restriction applies.' END END)
    END) ON CONFLICT(dedupe_key) DO NOTHING;
   current_state.notified_at:=now();
  END IF;
  changed:=previous.over_since IS DISTINCT FROM current_state.over_since OR previous.enforced_at IS DISTINCT FROM current_state.enforced_at;
  UPDATE tenant_limit_states SET plan_id=current_state.plan_id,over_since=current_state.over_since,
   grace_until=current_state.grace_until,enforced_at=current_state.enforced_at,
   notified_at=current_state.notified_at,warn_period=current_state.warn_period
   WHERE tenant_id=target AND resource=r.resource;
  IF changed THEN
   INSERT INTO tenant_limit_events(tenant_id,resource,action,detail) VALUES(target,r.resource,'decision',
    jsonb_build_object('over_since',current_state.over_since,'grace_until',current_state.grace_until,'enforced',current_state.enforced_at IS NOT NULL,'value',r.value,'allowance',r.allowance));
  END IF;
  IF current_state.enforced_at IS NOT NULL THEN
   block_users:=block_users OR r.resource='users'; block_media:=block_media OR r.resource='media';
   valid_until:=(r.sampled_at/1000)+1800;
  END IF;
 END LOOP;
 UPDATE tenants SET limit_registrations=NOT block_users,limit_media_uploads=NOT block_media,
  limits_valid_until=valid_until,config_revision=config_revision+1,reconcile_failures=0,reconcile_error=NULL,updated_at=now()
  WHERE id=target AND (limit_registrations IS DISTINCT FROM NOT block_users OR limit_media_uploads IS DISTINCT FROM NOT block_media OR limits_valid_until IS DISTINCT FROM valid_until);
 IF FOUND THEN
  UPDATE runtimes SET reconcile_requested=true WHERE id=(SELECT runtime_id FROM tenants WHERE id=target);
 END IF;
END $$;
CREATE FUNCTION refresh_user_limits() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE tid uuid;
BEGIN
 PERFORM 1 FROM tenants WHERE user_id=NEW.user_id ORDER BY id FOR UPDATE;
 FOR tid IN SELECT id FROM tenants WHERE user_id=NEW.user_id ORDER BY id LOOP PERFORM refresh_tenant_limits(tid); END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER subscription_limit_change AFTER INSERT OR UPDATE OF plan_id,status ON subscriptions
 FOR EACH ROW EXECUTE FUNCTION refresh_user_limits();
CREATE FUNCTION refresh_sample_limits() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN PERFORM refresh_tenant_limits(NEW.tenant_id); RETURN NEW; END $$;
CREATE TRIGGER usage_limit_change AFTER INSERT ON tenant_usage FOR EACH ROW EXECUTE FUNCTION refresh_sample_limits();
CREATE FUNCTION instance_over_limits(owner_id uuid,target_plan uuid) RETURNS jsonb LANGUAGE sql STABLE AS $$
 SELECT COALESCE(jsonb_agg(jsonb_build_object('tenant_id',r.tenant_id,'domain',r.domain,'resource',r.resource,'value',r.value,'allowance',v.allowance,'sampled_at',r.sampled_at)),'[]'::jsonb)
 FROM tenant_limit_readings r JOIN plans p ON p.id=target_plan
 CROSS JOIN LATERAL (SELECT CASE r.resource WHEN 'users' THEN p.user_limit::numeric WHEN 'media' THEN p.media_gb::numeric*1000000000 WHEN 'database' THEN p.database_gb::numeric*1000000000 ELSE p.monthly_activities::numeric END allowance) v
 WHERE r.user_id=owner_id AND r.lifecycle_state IN ('ready','failed') AND r.fresh AND NOT r.exempt AND v.allowance IS NOT NULL AND r.value>=v.allowance
$$;
CREATE TABLE tenant_usage_rechecks (
 tenant_id uuid PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
 requested_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION refresh_account_limit_flags() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE tid uuid;
BEGIN
 PERFORM 1 FROM tenants WHERE user_id=NEW.id ORDER BY id FOR UPDATE;
 FOR tid IN SELECT id FROM tenants WHERE user_id=NEW.id ORDER BY id LOOP PERFORM refresh_tenant_limits(tid); END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER account_limit_exemption AFTER UPDATE OF is_admin,billing_exempt ON users
 FOR EACH ROW EXECUTE FUNCTION refresh_account_limit_flags();
CREATE FUNCTION refresh_plan_limit_flags() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE tid uuid;
BEGIN
 PERFORM 1 FROM tenants t JOIN subscriptions s ON s.user_id=t.user_id WHERE s.plan_id=NEW.id ORDER BY t.id FOR UPDATE OF t;
 FOR tid IN SELECT t.id FROM tenants t JOIN subscriptions s ON s.user_id=t.user_id WHERE s.plan_id=NEW.id ORDER BY t.id LOOP PERFORM refresh_tenant_limits(tid); END LOOP;
 RETURN NEW;
END $$;
CREATE TRIGGER plan_allowance_change AFTER UPDATE OF user_limit,media_gb,database_gb,monthly_activities ON plans
 FOR EACH ROW EXECUTE FUNCTION refresh_plan_limit_flags();

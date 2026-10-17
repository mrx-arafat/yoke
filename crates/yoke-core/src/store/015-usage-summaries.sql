-- Exact default usage summaries; raw records remain available for detailed history.
CREATE INDEX usage_time ON usage_samples(ts);
CREATE INDEX usage_session_time ON usage_samples(agent,session_id,ts);
CREATE TABLE usage_session_totals (
 agent TEXT NOT NULL, session_id TEXT NOT NULL, project TEXT NOT NULL,
 first_at INTEGER, last_at INTEGER,
 samples INTEGER NOT NULL,
 input_tokens INTEGER NOT NULL,
 output_tokens INTEGER NOT NULL,
 cache_read_tokens INTEGER NOT NULL,
 cache_write_tokens INTEGER NOT NULL,
 cost_nano_usd INTEGER NOT NULL,
 unknown_pricing_samples INTEGER NOT NULL,
 PRIMARY KEY(agent,session_id)
) WITHOUT ROWID;
CREATE INDEX usage_session_recent ON usage_session_totals(last_at IS NULL,last_at DESC,session_id);
CREATE TABLE usage_session_models (agent TEXT NOT NULL,session_id TEXT NOT NULL,model TEXT NOT NULL,samples INTEGER NOT NULL,PRIMARY KEY(agent,session_id,model)) WITHOUT ROWID;
CREATE TABLE usage_day_totals (agent TEXT NOT NULL,day INTEGER NOT NULL,samples INTEGER NOT NULL,input_tokens INTEGER NOT NULL,output_tokens INTEGER NOT NULL,cache_read_tokens INTEGER NOT NULL,cache_write_tokens INTEGER NOT NULL,cost_nano_usd INTEGER NOT NULL,unknown_pricing_samples INTEGER NOT NULL,PRIMARY KEY(day,agent)) WITHOUT ROWID;
INSERT INTO usage_session_totals SELECT agent,session_id,project,min(ts),max(ts),count(*),sum(input_tokens),sum(output_tokens),sum(cache_read_tokens),sum(cache_write_tokens),coalesce(sum(cost_nano_usd),0),sum(cost_nano_usd IS NULL) FROM usage_samples GROUP BY agent,session_id;
INSERT INTO usage_session_models SELECT agent,session_id,model,count(*) FROM usage_samples GROUP BY agent,session_id,model;
INSERT INTO usage_day_totals SELECT agent,(usage_samples.ts - ((usage_samples.ts % 86400 + 86400) % 86400)),count(*),sum(input_tokens),sum(output_tokens),sum(cache_read_tokens),sum(cache_write_tokens),coalesce(sum(cost_nano_usd),0),sum(cost_nano_usd IS NULL) FROM usage_samples WHERE ts IS NOT NULL GROUP BY agent,(usage_samples.ts - ((usage_samples.ts % 86400 + 86400) % 86400));
CREATE TRIGGER usage_totals_insert AFTER INSERT ON usage_samples BEGIN
 INSERT INTO usage_session_totals VALUES(new.agent,new.session_id,new.project,new.ts,new.ts,1,new.input_tokens,new.output_tokens,new.cache_read_tokens,new.cache_write_tokens,coalesce(new.cost_nano_usd,0),new.cost_nano_usd IS NULL)
 ON CONFLICT(agent,session_id) DO UPDATE SET project=excluded.project,
 first_at=CASE WHEN first_at IS NULL THEN new.ts WHEN new.ts IS NULL THEN first_at ELSE min(first_at,new.ts) END,
 last_at=CASE WHEN last_at IS NULL THEN new.ts WHEN new.ts IS NULL THEN last_at ELSE max(last_at,new.ts) END,
 samples=samples+excluded.samples,input_tokens=input_tokens+excluded.input_tokens,output_tokens=output_tokens+excluded.output_tokens,cache_read_tokens=cache_read_tokens+excluded.cache_read_tokens,cache_write_tokens=cache_write_tokens+excluded.cache_write_tokens,cost_nano_usd=cost_nano_usd+excluded.cost_nano_usd,unknown_pricing_samples=unknown_pricing_samples+excluded.unknown_pricing_samples;
 INSERT INTO usage_session_models VALUES(new.agent,new.session_id,new.model,1)
 ON CONFLICT(agent,session_id,model) DO UPDATE SET samples=samples+1;
 INSERT INTO usage_day_totals SELECT new.agent,(new.ts - ((new.ts % 86400 + 86400) % 86400)),1,new.input_tokens,new.output_tokens,new.cache_read_tokens,new.cache_write_tokens,coalesce(new.cost_nano_usd,0),new.cost_nano_usd IS NULL WHERE new.ts IS NOT NULL
 ON CONFLICT(day,agent) DO UPDATE SET samples=samples+excluded.samples,input_tokens=input_tokens+excluded.input_tokens,output_tokens=output_tokens+excluded.output_tokens,cache_read_tokens=cache_read_tokens+excluded.cache_read_tokens,cache_write_tokens=cache_write_tokens+excluded.cache_write_tokens,cost_nano_usd=cost_nano_usd+excluded.cost_nano_usd,unknown_pricing_samples=unknown_pricing_samples+excluded.unknown_pricing_samples;
 END;

-- Preserve existing notification identities and acknowledgement while adding a condition.
DROP INDEX notifications_project;
ALTER TABLE notifications RENAME TO notifications_previous;
CREATE TABLE notifications (
 id INTEGER PRIMARY KEY, key TEXT NOT NULL UNIQUE,
 kind TEXT NOT NULL CHECK(kind IN ('hook_error','observe_would_deny','plan_waiting','plan_stalled','check_failed','config_stale','config_untrusted','usage_partial')),
 project TEXT NOT NULL, subject TEXT NOT NULL, plan_id INTEGER, title TEXT NOT NULL, detail TEXT NOT NULL,
 count INTEGER NOT NULL DEFAULT 1 CHECK(count>=1),first_seen INTEGER NOT NULL,last_seen INTEGER NOT NULL,
 state TEXT NOT NULL DEFAULT 'unread' CHECK(state IN ('unread','acknowledged')),acknowledged_at INTEGER,
 actor TEXT CHECK(actor IN ('human','yoke')),resolved_at INTEGER,
 CHECK((state='unread')=(acknowledged_at IS NULL AND actor IS NULL))
);
INSERT INTO notifications(id,key,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen,state,acknowledged_at,actor,resolved_at) SELECT id,key,kind,project,subject,plan_id,title,detail,count,first_seen,last_seen,state,acknowledged_at,actor,resolved_at FROM notifications_previous;
DROP TABLE notifications_previous;
CREATE INDEX notifications_project ON notifications(project,state,last_seen DESC);
CREATE TRIGGER usage_totals_delete AFTER DELETE ON usage_samples BEGIN
 UPDATE usage_session_totals SET samples=samples-(1),input_tokens=input_tokens-(old.input_tokens),output_tokens=output_tokens-(old.output_tokens),cache_read_tokens=cache_read_tokens-(old.cache_read_tokens),cache_write_tokens=cache_write_tokens-(old.cache_write_tokens),cost_nano_usd=cost_nano_usd-(coalesce(old.cost_nano_usd,0)),unknown_pricing_samples=unknown_pricing_samples-(old.cost_nano_usd IS NULL),
 first_at=CASE WHEN old.ts=first_at THEN (SELECT min(ts) FROM usage_samples WHERE agent=old.agent AND session_id=old.session_id) ELSE first_at END,
 last_at=CASE WHEN old.ts=last_at THEN (SELECT max(ts) FROM usage_samples WHERE agent=old.agent AND session_id=old.session_id) ELSE last_at END
 WHERE agent=old.agent AND session_id=old.session_id;
 DELETE FROM usage_session_totals WHERE agent=old.agent AND session_id=old.session_id AND samples=0;
 UPDATE usage_session_models SET samples=samples-1 WHERE agent=old.agent AND session_id=old.session_id AND model=old.model;
 DELETE FROM usage_session_models WHERE agent=old.agent AND session_id=old.session_id AND model=old.model AND samples=0;
 UPDATE usage_day_totals SET samples=samples-(1),input_tokens=input_tokens-(old.input_tokens),output_tokens=output_tokens-(old.output_tokens),cache_read_tokens=cache_read_tokens-(old.cache_read_tokens),cache_write_tokens=cache_write_tokens-(old.cache_write_tokens),cost_nano_usd=cost_nano_usd-(coalesce(old.cost_nano_usd,0)),unknown_pricing_samples=unknown_pricing_samples-(old.cost_nano_usd IS NULL) WHERE day=(old.ts - ((old.ts % 86400 + 86400) % 86400)) AND agent=old.agent;
 DELETE FROM usage_day_totals WHERE day=(old.ts - ((old.ts % 86400 + 86400) % 86400)) AND agent=old.agent AND samples=0;
 END;
CREATE TRIGGER usage_totals_update AFTER UPDATE ON usage_samples
WHEN old.agent IS NOT new.agent OR old.session_id IS NOT new.session_id OR old.project IS NOT new.project OR old.model IS NOT new.model OR old.ts IS NOT new.ts OR old.input_tokens IS NOT new.input_tokens OR old.output_tokens IS NOT new.output_tokens OR old.cache_read_tokens IS NOT new.cache_read_tokens OR old.cache_write_tokens IS NOT new.cache_write_tokens OR old.cost_nano_usd IS NOT new.cost_nano_usd
BEGIN
 UPDATE usage_session_totals SET samples=samples-(1),input_tokens=input_tokens-(old.input_tokens),output_tokens=output_tokens-(old.output_tokens),cache_read_tokens=cache_read_tokens-(old.cache_read_tokens),cache_write_tokens=cache_write_tokens-(old.cache_write_tokens),cost_nano_usd=cost_nano_usd-(coalesce(old.cost_nano_usd,0)),unknown_pricing_samples=unknown_pricing_samples-(old.cost_nano_usd IS NULL),
 first_at=CASE WHEN old.ts=first_at THEN (SELECT min(ts) FROM usage_samples WHERE agent=old.agent AND session_id=old.session_id) ELSE first_at END,
 last_at=CASE WHEN old.ts=last_at THEN (SELECT max(ts) FROM usage_samples WHERE agent=old.agent AND session_id=old.session_id) ELSE last_at END
 WHERE agent=old.agent AND session_id=old.session_id;
 DELETE FROM usage_session_totals WHERE agent=old.agent AND session_id=old.session_id AND samples=0;
 UPDATE usage_session_models SET samples=samples-1 WHERE agent=old.agent AND session_id=old.session_id AND model=old.model;
 DELETE FROM usage_session_models WHERE agent=old.agent AND session_id=old.session_id AND model=old.model AND samples=0;
 UPDATE usage_day_totals SET samples=samples-(1),input_tokens=input_tokens-(old.input_tokens),output_tokens=output_tokens-(old.output_tokens),cache_read_tokens=cache_read_tokens-(old.cache_read_tokens),cache_write_tokens=cache_write_tokens-(old.cache_write_tokens),cost_nano_usd=cost_nano_usd-(coalesce(old.cost_nano_usd,0)),unknown_pricing_samples=unknown_pricing_samples-(old.cost_nano_usd IS NULL) WHERE day=(old.ts - ((old.ts % 86400 + 86400) % 86400)) AND agent=old.agent;
 DELETE FROM usage_day_totals WHERE day=(old.ts - ((old.ts % 86400 + 86400) % 86400)) AND agent=old.agent AND samples=0;

 INSERT INTO usage_session_totals VALUES(new.agent,new.session_id,new.project,new.ts,new.ts,1,new.input_tokens,new.output_tokens,new.cache_read_tokens,new.cache_write_tokens,coalesce(new.cost_nano_usd,0),new.cost_nano_usd IS NULL)
 ON CONFLICT(agent,session_id) DO UPDATE SET project=excluded.project,
 first_at=CASE WHEN first_at IS NULL THEN new.ts WHEN new.ts IS NULL THEN first_at ELSE min(first_at,new.ts) END,
 last_at=CASE WHEN last_at IS NULL THEN new.ts WHEN new.ts IS NULL THEN last_at ELSE max(last_at,new.ts) END,
 samples=samples+excluded.samples,input_tokens=input_tokens+excluded.input_tokens,output_tokens=output_tokens+excluded.output_tokens,cache_read_tokens=cache_read_tokens+excluded.cache_read_tokens,cache_write_tokens=cache_write_tokens+excluded.cache_write_tokens,cost_nano_usd=cost_nano_usd+excluded.cost_nano_usd,unknown_pricing_samples=unknown_pricing_samples+excluded.unknown_pricing_samples;
 INSERT INTO usage_session_models VALUES(new.agent,new.session_id,new.model,1)
 ON CONFLICT(agent,session_id,model) DO UPDATE SET samples=samples+1;
 INSERT INTO usage_day_totals SELECT new.agent,(new.ts - ((new.ts % 86400 + 86400) % 86400)),1,new.input_tokens,new.output_tokens,new.cache_read_tokens,new.cache_write_tokens,coalesce(new.cost_nano_usd,0),new.cost_nano_usd IS NULL WHERE new.ts IS NOT NULL
 ON CONFLICT(day,agent) DO UPDATE SET samples=samples+excluded.samples,input_tokens=input_tokens+excluded.input_tokens,output_tokens=output_tokens+excluded.output_tokens,cache_read_tokens=cache_read_tokens+excluded.cache_read_tokens,cache_write_tokens=cache_write_tokens+excluded.cache_write_tokens,cost_nano_usd=cost_nano_usd+excluded.cost_nano_usd,unknown_pricing_samples=unknown_pricing_samples+excluded.unknown_pricing_samples;
 END;

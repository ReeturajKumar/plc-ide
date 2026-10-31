-- Usage-driven st_tags (Option A). st_tags stops being a mirror of tagmast and instead holds
-- only the tags programs actually use, each with a usage count and a per-project breakdown.
-- MyPLC writes ONLY this table. Available tags now come from tagmast (see tags::load()).
--
-- Run order for a safe rollout:
--   1. The ALTER TABLE below is additive and safe to run any time (old code ignores the columns).
--   2. The DROP TRIGGER / DROP FUNCTION / DELETE are destructive: run them only once the
--      usage-driven backend is deployed, or the live IDE briefly sees no tags.

BEGIN;

-- 1. Additive, safe: the usage columns.
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS usage_count integer NOT NULL DEFAULT 0;
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS usage_by_project jsonb NOT NULL DEFAULT '{}'::jsonb;

-- Durable last-known value per tag (snapshotted by the backend, seeded on restart).
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS last_value double precision;
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS last_value_at timestamptz;

COMMIT;

-- 2. Destructive (run at deploy time): stop the old tagmast -> st_tags mirror and start empty.
--    Uncomment and run once the usage-driven backend is live.
-- BEGIN;
-- DROP TRIGGER IF EXISTS trg_sync_st_tags ON public.tagmast;
-- DROP TRIGGER IF EXISTS trg_truncate_st_tags ON public.tagmast;
-- DROP FUNCTION IF EXISTS public.fn_sync_st_tags();
-- DROP FUNCTION IF EXISTS public.fn_truncate_st_tags();
-- DELETE FROM public.st_tags;   -- st_tags refills as programs are compiled
-- COMMIT;

-- st_tags: the SCADA tags MyPLC programs can use, one row per tag of tagmast:
--   id = tagmast.id, tagid = tagmast.tag_id (the ID the SCADA's live messages carry),
--   tag_name, tag_class = tagmast.tag_type, data_type = tagmast.data_type (a code
--   explained by control_table, group 5: 54 Binary, 57 Float ABCD, …).
-- Kept in sync by a trigger on tagmast that touches only the changed row, and only when
-- one of those columns changes (not on quality / alarm updates). It never fails, so it
-- can't block the SCADA's own writes. Safe to run again.

BEGIN;

CREATE TABLE IF NOT EXISTS public.st_tags (
    id        bigint PRIMARY KEY,           -- tagmast.id
    tagid     character varying(100),       -- tagmast.tag_id
    tag_name  character varying(250),       -- tagmast.tag_name
    tag_class character varying(100),       -- tagmast.tag_type
    data_type character varying(100)        -- tagmast.data_type
);
-- Earlier versions: tagid was a uuid, data_type an integer, tag_name required.
ALTER TABLE public.st_tags ALTER COLUMN tagid TYPE character varying(100) USING tagid::text;
ALTER TABLE public.st_tags ALTER COLUMN data_type TYPE character varying(100) USING data_type::text;
ALTER TABLE public.st_tags ALTER COLUMN tag_name DROP NOT NULL;
CREATE INDEX IF NOT EXISTS st_tags_tagid ON public.st_tags (tagid);

CREATE OR REPLACE FUNCTION public.fn_sync_st_tags() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP IN ('DELETE', 'UPDATE') THEN
        DELETE FROM public.st_tags WHERE id = OLD.id;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        INSERT INTO public.st_tags (id, tagid, tag_name, tag_class, data_type)
        VALUES (NEW.id, NEW.tag_id, NEW.tag_name, NEW.tag_type, NEW.data_type)
        ON CONFLICT (id) DO UPDATE
            SET tagid = EXCLUDED.tagid, tag_name = EXCLUDED.tag_name,
                tag_class = EXCLUDED.tag_class, data_type = EXCLUDED.data_type;
    END IF;
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION public.fn_truncate_st_tags() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    TRUNCATE public.st_tags;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS trg_sync_st_tags ON public.tagmast;
CREATE TRIGGER trg_sync_st_tags
    AFTER INSERT OR DELETE OR UPDATE OF id, tag_id, tag_name, tag_type, data_type ON public.tagmast
    FOR EACH ROW EXECUTE FUNCTION public.fn_sync_st_tags();

DROP TRIGGER IF EXISTS trg_truncate_st_tags ON public.tagmast;
CREATE TRIGGER trg_truncate_st_tags
    AFTER TRUNCATE ON public.tagmast
    FOR EACH STATEMENT EXECUTE FUNCTION public.fn_truncate_st_tags();

-- An earlier version also synced from scada_master_tags; st_tags no longer needs it.
DROP TRIGGER IF EXISTS trg_sync_st_tags ON public.scada_master_tags;

-- Fill it now.
DELETE FROM public.st_tags;
INSERT INTO public.st_tags (id, tagid, tag_name, tag_class, data_type)
SELECT id, tag_id, tag_name, tag_type, data_type FROM public.tagmast;

COMMIT;

-- st_tags: the SCADA tags MyPLC programs can use, one row per tag of tagmast (id, tagid,
-- tag_name, data_type as tagmast has them) with the tag class from scada_master_tags
-- (linked by tagmast.uuid = scada_master_tags.source_tag_id). Kept in sync by triggers
-- on both tables. MyPLC takes the type its programs see (BOOL / REAL) from
-- scada_master_tags.data_type, since tagmast.data_type is a code (1 for every tag).
-- Safe to run again.

BEGIN;

CREATE TABLE IF NOT EXISTS public.st_tags (
    id        bigint PRIMARY KEY,                 -- tagmast.id
    tagid     uuid,                               -- tagmast.tagid
    tag_name  character varying(250) NOT NULL,    -- tagmast.tag_name
    tag_class character varying(100),             -- scada_master_tags.tag_class
    data_type integer                             -- tagmast.data_type
);

-- An earlier version stored scada_master_tags.data_type (text) here.
ALTER TABLE public.st_tags ALTER COLUMN data_type TYPE integer USING NULL;

-- Rebuild st_tags from the two source tables.
-- ponytail: rebuilds the whole table per change; fine for hundreds of tags, switch to a
-- per-row upsert if there are many thousands.
CREATE OR REPLACE FUNCTION public.fn_sync_st_tags() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM public.st_tags;
    INSERT INTO public.st_tags (id, tagid, tag_name, tag_class, data_type)
    SELECT t.id, t.tagid, t.tag_name, s.tag_class, t.data_type
    FROM public.tagmast t
    LEFT JOIN public.scada_master_tags s ON s.source_tag_id = t.uuid::text;
    RETURN NULL;
END;
$$;

DROP TRIGGER IF EXISTS trg_sync_st_tags ON public.tagmast;
CREATE TRIGGER trg_sync_st_tags
    AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON public.tagmast
    FOR EACH STATEMENT EXECUTE FUNCTION public.fn_sync_st_tags();

DROP TRIGGER IF EXISTS trg_sync_st_tags ON public.scada_master_tags;
CREATE TRIGGER trg_sync_st_tags
    AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON public.scada_master_tags
    FOR EACH STATEMENT EXECUTE FUNCTION public.fn_sync_st_tags();

-- Fill it now.
DELETE FROM public.st_tags;
INSERT INTO public.st_tags (id, tagid, tag_name, tag_class, data_type)
SELECT t.id, t.tagid, t.tag_name, s.tag_class, t.data_type
FROM public.tagmast t
LEFT JOIN public.scada_master_tags s ON s.source_tag_id = t.uuid::text;

COMMIT;

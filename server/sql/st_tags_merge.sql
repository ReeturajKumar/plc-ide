-- Complete st_tags table for the usage-driven backend. Safe to run whether or not the table
-- exists, and safe to run twice. Keeps all existing rows and the old sync triggers.
-- If anything fails, the whole script rolls back and nothing changes.

BEGIN;

-- Creates the table if it doesn't exist
CREATE TABLE IF NOT EXISTS public.st_tags (
    id               text PRIMARY KEY,                    -- tagmast.id (an integer or a uuid)
    tagid            character varying(100),              -- tagmast.tag_id
    tag_name         character varying(250),              -- tagmast.tag_name
    tag_class        character varying(100),              -- tagmast.tag_type
    data_type        character varying(100),              -- tagmast.data_type
    protocol_id      character varying(100),              -- tagmast.protocol_type
    usage_count     integer NOT NULL DEFAULT 0,          -- total uses across all projects
    usage_by_project jsonb NOT NULL DEFAULT '{}'::jsonb,  -- uses per project
    last_value       double precision,                    -- last saved value
    last_value_at    timestamptz                          -- when it was saved
);

-- Adds the new columns if the table already exists
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS usage_count integer NOT NULL DEFAULT 0;
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS usage_by_project jsonb NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS last_value double precision;
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS last_value_at timestamptz;
ALTER TABLE public.st_tags ADD COLUMN IF NOT EXISTS protocol_id character varying(100);
ALTER TABLE public.st_tags DROP COLUMN IF EXISTS protocol;   -- replaced by protocol_id

-- Fills the protocol id for rows that are already there (new rows get it from the backend)
UPDATE public.st_tags st SET protocol_id = t.protocol_type
FROM public.tagmast t WHERE t.id::text = st.id AND st.protocol_id IS NULL;

-- Brings older versions of the table up to date
-- tagmast.id is an integer in some databases and a uuid in others: text holds both.
-- (An auto-numbered id can't be text, so the auto-numbering is dropped first; the id comes from tagmast anyway.)
ALTER TABLE public.st_tags ALTER COLUMN id DROP IDENTITY IF EXISTS;
ALTER TABLE public.st_tags ALTER COLUMN id DROP DEFAULT;
ALTER TABLE public.st_tags ALTER COLUMN id TYPE text USING id::text;
ALTER TABLE public.st_tags ALTER COLUMN tagid TYPE character varying(100) USING tagid::text;
ALTER TABLE public.st_tags ALTER COLUMN data_type TYPE character varying(100) USING data_type::text;
ALTER TABLE public.st_tags ALTER COLUMN tag_name DROP NOT NULL;

-- The backend's inserts need id to be the primary key
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'public.st_tags'::regclass AND contype = 'p'
    ) THEN
        ALTER TABLE public.st_tags ADD PRIMARY KEY (id);
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS st_tags_tagid ON public.st_tags (tagid);

COMMIT;

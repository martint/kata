-- Optional intra-line column range for comments. Only valid for
-- single-line line-level comments (the upsert path's validation
-- rejects multi-line column ranges); NULL otherwise. UTF-16 offsets
-- — frontend's drag-to-select arithmetic does its conversion in
-- those units, so storing them raw avoids per-read encoding work.
ALTER TABLE comments ADD COLUMN col_start INTEGER;
ALTER TABLE comments ADD COLUMN col_end INTEGER;

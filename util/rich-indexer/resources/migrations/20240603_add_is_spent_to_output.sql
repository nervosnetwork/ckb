-- 20240630_add_is_spent_to_output.sql

ALTER TABLE output
ADD COLUMN is_spent INTEGER DEFAULT 0;

UPDATE output
SET is_spent = 1
FROM input
WHERE input.output_id = output.id;

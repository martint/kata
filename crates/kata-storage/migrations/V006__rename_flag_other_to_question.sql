-- The "other" flag has been renamed to "question". The data model never
-- used "other" for anything but questions; the rename just makes the
-- intent explicit (and clears the way for the UI's "responders shouldn't
-- auto-resolve questions" rule). Existing rows with flag = 'other' are
-- carried forward as 'question' so old comments keep their classification.
UPDATE comments SET flag = 'question' WHERE flag = 'other';

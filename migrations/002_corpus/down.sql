-- Rollback for Migration 002: Corpus table
-- Reverses: CREATE TABLE _spindle_corpus, CREATE INDEX x2, INSERT seed data

DROP INDEX IF EXISTS idx_corpus_type;
DROP INDEX IF EXISTS idx_corpus_author;
DROP TABLE IF EXISTS _spindle_corpus;

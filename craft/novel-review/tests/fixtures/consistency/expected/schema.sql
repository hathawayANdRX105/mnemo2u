CREATE TABLE documents (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            doc_type TEXT NOT NULL,
            arc TEXT,
            story TEXT,
            chapter TEXT
        );
CREATE TABLE passages (
            id INTEGER PRIMARY KEY,
            document_id INTEGER NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            text TEXT NOT NULL,
            FOREIGN KEY(document_id) REFERENCES documents(id)
        );
CREATE VIRTUAL TABLE passage_fts USING fts5(
            path UNINDEXED,
            text,
            content=''
        );
CREATE TABLE 'passage_fts_data'(id INTEGER PRIMARY KEY, block BLOB);
CREATE TABLE 'passage_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE 'passage_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE 'passage_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID;
CREATE TABLE entities (
            id INTEGER PRIMARY KEY,
            category TEXT NOT NULL,
            card_path TEXT NOT NULL,
            card_id TEXT,
            title TEXT NOT NULL
        );
CREATE TABLE entity_names (
            id INTEGER PRIMARY KEY,
            entity_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            FOREIGN KEY(entity_id) REFERENCES entities(id)
        );
CREATE TABLE mentions (
            id INTEGER PRIMARY KEY,
            entity_id INTEGER NOT NULL,
            entity_name_id INTEGER NOT NULL,
            document_id INTEGER NOT NULL,
            passage_id INTEGER NOT NULL,
            count INTEGER NOT NULL,
            FOREIGN KEY(entity_id) REFERENCES entities(id),
            FOREIGN KEY(entity_name_id) REFERENCES entity_names(id),
            FOREIGN KEY(document_id) REFERENCES documents(id),
            FOREIGN KEY(passage_id) REFERENCES passages(id)
        );
CREATE TABLE fact_candidates (
            id INTEGER PRIMARY KEY,
            entity_id INTEGER NOT NULL,
            entity_name_id INTEGER NOT NULL,
            document_id INTEGER NOT NULL,
            passage_id INTEGER NOT NULL,
            fact_type TEXT NOT NULL,
            cue TEXT NOT NULL,
            FOREIGN KEY(entity_id) REFERENCES entities(id),
            FOREIGN KEY(entity_name_id) REFERENCES entity_names(id),
            FOREIGN KEY(document_id) REFERENCES documents(id),
            FOREIGN KEY(passage_id) REFERENCES passages(id)
        );
CREATE INDEX idx_passages_document ON passages(document_id);
CREATE INDEX idx_mentions_entity ON mentions(entity_id);
CREATE INDEX idx_mentions_document ON mentions(document_id);
CREATE INDEX idx_entity_names_name ON entity_names(name);
CREATE INDEX idx_fact_candidates_entity ON fact_candidates(entity_id);
CREATE INDEX idx_fact_candidates_type ON fact_candidates(fact_type);

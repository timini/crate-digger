-- Ready-queue size over time, to report how often tracks were ready.
CREATE TABLE buffer_sample (
    at    INTEGER NOT NULL,
    ready INTEGER NOT NULL
);
CREATE INDEX buffer_sample_at ON buffer_sample(at);

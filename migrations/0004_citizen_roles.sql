ALTER TABLE citizens ADD COLUMN role BIGINT NOT NULL DEFAULT 0;

-- bit 1 = citizen
-- bit 2 = minister
-- bit 3 = census minister
-- bit 4 = election minister
-- bit 5 = admin
-- bit 6 = superadmin
-- bit 7-64 = reserved for future use
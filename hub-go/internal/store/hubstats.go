package store

// AssetBytes is the stored size of every asset blob (each distinct upload counted once).
func (s *Store) AssetBytes() (int64, error) {
	var n int64
	err := s.db.QueryRow(`SELECT COALESCE(SUM(size), 0) FROM asset_blobs`).Scan(&n)
	return n, err
}

package devplatform

import (
	"go.mongodb.org/mongo-driver/v2/bson"
	"path/filepath"
	"testing"
)

func TestMongoBackupValidatesMetadataBeforeRestore(t *testing.T) {
	path := filepath.Join(t.TempDir(), "backup.dump")
	doc, e := bson.Marshal(bson.D{{Key: "n", Value: int32(42)}})
	if e != nil {
		t.Fatal(e)
	}
	legacy := mongoBackupFile{Schema: 1, Collections: map[string][]bson.Raw{"sample": {doc}}}
	if e = writeJSON(path, legacy); e != nil {
		t.Fatal(e)
	}
	if _, e = readMongoBackup(path); e != nil {
		t.Fatal("legacy BSON backup is no longer readable", e)
	}
	invalidIndex, _ := bson.Marshal(bson.D{{Key: "name", Value: 17}})
	legacy.Schema = 2
	legacy.Metadata = map[string]mongoCollectionMetadata{"sample": {Type: "collection", Indexes: []bson.Raw{invalidIndex}}}
	if e = writeJSON(path, legacy); e != nil {
		t.Fatal(e)
	}
	if _, e = readMongoBackup(path); e == nil {
		t.Fatal("invalid index metadata was accepted")
	}
}

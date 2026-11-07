package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"go.mongodb.org/mongo-driver/v2/bson"
	"io"
	"os"
	"sort"
	"strings"
)

// Raw BSON preserves ObjectId, date and numeric types. Schema 1 remains readable.
type mongoBackupFile struct {
	Schema      int                                `json:"schema"`
	Collections map[string][]bson.Raw              `json:"collections"`
	Metadata    map[string]mongoCollectionMetadata `json:"metadata,omitempty"`
}
type mongoCollectionMetadata struct {
	Type    string     `json:"type"`
	Options bson.Raw   `json:"options,omitempty"`
	Indexes []bson.Raw `json:"indexes,omitempty"`
}

const mongoDocumentLimit = 256 << 20

func (m *Manager) mongoBackup(ctx context.Context, i Instance, name, path string) error {
	client, e := m.mongoClient(ctx, i, true)
	if e != nil {
		return e
	}
	defer client.Disconnect(context.Background())
	db := client.Database(name)
	cursor, e := db.ListCollections(ctx, bson.D{})
	if e != nil {
		return e
	}
	var collections []struct {
		Name    string   `bson:"name"`
		Type    string   `bson:"type"`
		Options bson.Raw `bson:"options"`
	}
	if e = cursor.All(ctx, &collections); e != nil {
		return e
	}
	backup := mongoBackupFile{Schema: 2, Collections: map[string][]bson.Raw{}, Metadata: map[string]mongoCollectionMetadata{}}
	total := 0
	for _, collection := range collections {
		if strings.HasPrefix(collection.Name, "system.") {
			continue
		}
		meta := mongoCollectionMetadata{Type: collection.Type, Options: collection.Options}
		total += len(meta.Options)
		backup.Collections[collection.Name] = nil
		if collection.Type != "view" {
			indexes, e := db.Collection(collection.Name).Indexes().List(ctx)
			if e != nil {
				return e
			}
			if e = indexes.All(ctx, &meta.Indexes); e != nil {
				return e
			}
			for _, index := range meta.Indexes {
				total += len(index)
			}
			rows, e := db.Collection(collection.Name).Find(ctx, bson.D{})
			if e != nil {
				return e
			}
			for rows.Next(ctx) {
				total += len(rows.Current)
				if total > mongoDocumentLimit {
					rows.Close(ctx)
					return errors.New("MongoDB logical backup exceeds the 256 MiB document limit")
				}
				backup.Collections[collection.Name] = append(backup.Collections[collection.Name], append(bson.Raw(nil), rows.Current...))
			}
			e = rows.Err()
			rows.Close(ctx)
			if e != nil {
				return e
			}
		}
		backup.Metadata[collection.Name] = meta
	}
	return writeJSON(path, backup)
}

func readMongoBackup(path string) (mongoBackupFile, error) {
	f, e := os.Open(path)
	if e != nil {
		return mongoBackupFile{}, e
	}
	defer f.Close()
	info, e := f.Stat()
	if e != nil {
		return mongoBackupFile{}, e
	}
	if info.Size() > 512<<20 {
		return mongoBackupFile{}, errors.New("MongoDB backup file exceeds 512 MiB")
	}
	var backup mongoBackupFile
	decoder := json.NewDecoder(io.LimitReader(f, 512<<20))
	if e = decoder.Decode(&backup); e != nil {
		return backup, e
	}
	if (backup.Schema != 1 && backup.Schema != 2) || backup.Collections == nil {
		return backup, errors.New("unsupported MongoDB backup schema")
	}
	var trailing any
	if e = decoder.Decode(&trailing); e != io.EOF {
		return backup, errors.New("unexpected trailing backup data")
	}
	for name, docs := range backup.Collections {
		if name == "" || strings.ContainsRune(name, 0) || strings.HasPrefix(name, "system.") {
			return backup, errors.New("invalid backup collection name")
		}
		for _, doc := range docs {
			if e = doc.Validate(); e != nil {
				return backup, e
			}
		}
		meta, ok := backup.Metadata[name]
		if backup.Schema == 2 && (!ok || (meta.Type != "collection" && meta.Type != "timeseries" && meta.Type != "view")) {
			return backup, errors.New("missing or unsupported collection metadata")
		}
		if len(meta.Options) > 0 {
			if e = meta.Options.Validate(); e != nil {
				return backup, e
			}
		}
		for _, index := range meta.Indexes {
			if e = index.Validate(); e != nil {
				return backup, e
			}
			if name, ok := index.Lookup("name").StringValueOK(); !ok || name == "" {
				return backup, errors.New("invalid backup index name")
			}
			if _, ok := index.Lookup("key").DocumentOK(); !ok {
				return backup, errors.New("invalid backup index keys")
			}
		}
	}
	return backup, nil
}

func (m *Manager) mongoRestore(ctx context.Context, i Instance, name, path string) error {
	backup, e := readMongoBackup(path)
	if e != nil {
		return e
	}
	client, e := m.mongoClient(ctx, i, true)
	if e != nil {
		return e
	}
	defer client.Disconnect(context.Background())
	names, e := client.ListDatabaseNames(ctx, bson.D{{Key: "name", Value: name}})
	if e != nil {
		return e
	}
	if len(names) > 0 {
		return errors.New("restore target already exists")
	}
	db := client.Database(name)
	var collections []string
	for name := range backup.Collections {
		collections = append(collections, name)
	}
	sort.Slice(collections, func(a, b int) bool {
		av, bv := backup.Metadata[collections[a]].Type == "view", backup.Metadata[collections[b]].Type == "view"
		if av != bv {
			return !av
		}
		return collections[a] < collections[b]
	})
	for _, collection := range collections {
		meta := backup.Metadata[collection]
		command := bson.D{{Key: "create", Value: collection}}
		var settings bson.D
		if len(meta.Options) > 0 {
			if e = bson.Unmarshal(meta.Options, &settings); e != nil {
				return e
			}
		}
		command = append(command, settings...)
		if e = db.RunCommand(ctx, command).Err(); e != nil {
			return fmt.Errorf("create restored collection %s: %w", collection, e)
		}
		if docs := backup.Collections[collection]; len(docs) > 0 {
			if _, e = db.Collection(collection).InsertMany(ctx, docs); e != nil {
				return e
			}
		}
		var indexes bson.A
		for _, raw := range meta.Indexes {
			var fields bson.D
			if e = bson.Unmarshal(raw, &fields); e != nil {
				return e
			}
			if raw.Lookup("name").StringValue() == "_id_" {
				continue
			}
			clean := bson.D{}
			for _, field := range fields {
				if field.Key != "ns" && field.Key != "v" {
					clean = append(clean, field)
				}
			}
			indexes = append(indexes, clean)
		}
		if len(indexes) > 0 {
			if e = db.RunCommand(ctx, bson.D{{Key: "createIndexes", Value: collection}, {Key: "indexes", Value: indexes}}).Err(); e != nil {
				return e
			}
		}
	}
	return nil
}

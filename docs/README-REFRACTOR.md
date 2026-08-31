# Proposition de refonte documentaire Memoria

Fichiers nouveaux:
- SECURITY.md
- ARCHITECTURE.md
- RECOVERY.md

Fichiers proposés en remplacement:
- ASSURANCE.md
- ROADMAP.md
- AGENTS.md

KNOWLEDGE.md et WORKLOG.md ne sont pas réécrits dans cette première passe.

Décisions structurantes proposées:
1. Modèle source/acquisition/provenance conceptuel, sans trait Rust ni schéma SQL imposé.
2. Provenance directement attestée / déclarée par intermédiaire / contenu observé / dérivé / inconnue.
3. Ces catégories sont composables par assertion et ne forment jamais un niveau global du record.
4. L'ancien R2.3 sort de la numérotation recovery et devient M1, chantier architectural du modèle persistant acquisition/provenance; le salvage est un cas.
5. R2.2a suffit comme socle d'assurance pour commencer M1; batch/UX R2.2 non bloquant.
6. SECURITY.md sépare threat model et capacités de sécurité de l'assurance Tier A/B/C.
7. AGENTS.md adopte un cold-start routé pour réduire le coût contextuel.

# Vision and Principles

**Status:** Draft for public discussion. This document describes an intended project, not a completed product.

## Why this project exists

Open Social P2P aims to provide an approachable, open-source alternative to major social networks. People should be able to share conversations, photographs, communities and video without requiring one company to store everyone's media, control access to their social graph, or dictate how they may interact.

For ordinary members, joining and using the network should be familiar: sign up once, find people, follow or friend them, participate in groups, watch videos, and publish content. Federation and peer-to-peer networking should generally work behind the scenes.

## Core principles

1. **Easy for everyone.** No required server selection, networking setup, or knowledge of ActivityPub at registration.
2. **Open interoperability.** Federate with compatible services through open standards rather than locking people inside one application.
3. **User-owned media.** Support a desktop companion that stores originals on the user's own computer or other storage they select.
4. **Shared distribution.** People who share permitted public content can voluntarily help host it; popularity should improve available distribution capacity.
5. **Privacy by design.** Separate public distribution from access-controlled private content; minimize metadata collection and avoid exposing home IP addresses unnecessarily.
6. **User control.** Explicit limits for storage, bandwidth, replication, visibility and node participation; clear export and migration paths.
7. **Transparent behaviour.** Explain moderation, peer selection, replication, caching and algorithmic ordering. Prefer understandable user controls.
8. **Resilience.** Design for peers joining and leaving frequently, limited upload speeds, home routers and intermittent connectivity.
9. **Accessible and inclusive.** Accessibility, low bandwidth support and affordable participation are product requirements, not polish.

## Distinct projects

A separate *personal search engine with a crawler hosted in each user's home* has been discussed. **It is not part of Open Social P2P**, and this repository must not quietly merge those projects.

## Non-goals for the initial stage

- Writing production application code before agreeing on architecture and requirements.
- Creating a proprietary protocol where mature open standards can be reused.
- Assuming direct peer-to-peer networking can hide IP addresses.
- Promising that publicly replicated content can always be erased everywhere.
- Making a desktop computer mandatory to open a social account.

## Status

This repository currently documents the concept, questions and milestones. Proposed components and standards are subject to technical validation and community review.

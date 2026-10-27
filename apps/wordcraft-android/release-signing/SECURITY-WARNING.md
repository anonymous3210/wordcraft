# Security warning: public Android signing material

The keystore and its password in this directory are intentionally public at the repository owner's explicit direction. Treat this signing identity as compromised: anyone can download it and sign APKs as WordCraft.

Do not reuse this key for a store-distributed or otherwise trusted release. If the project needs a trusted signing identity, provision a new key and keep both the keystore and password in protected GitHub Actions secrets or another private signing service. Rotating the key may prevent existing installations from accepting updates signed with the new identity, so plan that migration deliberately.

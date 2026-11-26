/* Wrapper header para bindgen: expone la API de PJSUA que usamos.
   Redefine PJ_DECL para que bindgen genere funciones extern "C". */
#ifndef PJSIP_WRAPPER_H
#define PJSIP_WRAPPER_H

/* Forzar PJ_DECL a expandirse a "extern" para bindgen */
#ifdef PJ_DECL
#undef PJ_DECL
#endif
#define PJ_DECL(type) extern type

#include <pjsua-lib/pjsua.h>

#endif /* PJSIP_WRAPPER_H */

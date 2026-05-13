function decodeCommonEntities(value: string): string {
	return value
		.replace(/&nbsp;/gi, ' ')
		.replace(/&amp;/gi, '&')
		.replace(/&quot;/gi, '"')
		.replace(/&#39;/g, "'")
		.replace(/&lt;/gi, '<')
		.replace(/&gt;/gi, '>');
}

export function cleanArtistBio(value: string | null | undefined): string | null {
	if (!value) return null;
	const cleaned = decodeCommonEntities(value)
		.replace(/\[wimpLink[^\]]*\]([^\[]*)\[\/wimpLink\]/g, '$1')
		.replace(/<br\s*\/?>/gi, '\n')
		.replace(/<\/p>\s*<p[^>]*>/gi, '\n\n')
		.replace(/<\/?p[^>]*>/gi, '')
		.replace(/<[^>]+>/g, '')
		.replace(/[ \t]+\n/g, '\n')
		.replace(/\n[ \t]+/g, '\n')
		.replace(/[ \t]{2,}/g, ' ')
		.replace(/\n{3,}/g, '\n\n')
		.trim();
	return cleaned.length ? cleaned : null;
}

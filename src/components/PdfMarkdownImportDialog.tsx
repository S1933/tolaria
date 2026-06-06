import { useState } from 'react'
import type { AppLocale } from '../lib/i18n'
import { translate } from '../lib/i18n'
import type { PdfMarkdownOcrMode } from '../utils/pdfMarkdownImport'
import { Button } from './ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from './ui/dialog'
import { Input } from './ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from './ui/select'

interface PdfMarkdownImportDialogProps {
  fileTitle?: string
  locale?: AppLocale
  onClose: () => void
  onSubmit: (options: { ocrLanguage: string; ocrMode: PdfMarkdownOcrMode }) => void
  open: boolean
  working?: boolean
}

export function PdfMarkdownImportDialog({
  fileTitle,
  locale = 'en',
  onClose,
  onSubmit,
  open,
  working = false,
}: PdfMarkdownImportDialogProps) {
  const [ocrMode, setOcrMode] = useState<PdfMarkdownOcrMode>('ocr_when_needed')
  const [ocrLanguage, setOcrLanguage] = useState('eng')

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => { if (!nextOpen && !working) onClose() }}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{translate(locale, 'pdfImport.dialog.title')}</DialogTitle>
          <DialogDescription>
            {translate(locale, 'pdfImport.dialog.description', { file: fileTitle ?? translate(locale, 'pdfImport.dialog.untitledFile') })}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium text-foreground" htmlFor="pdf-import-mode">
              {translate(locale, 'pdfImport.mode.label')}
            </label>
            <Select value={ocrMode} onValueChange={(value) => setOcrMode(value as PdfMarkdownOcrMode)} disabled={working}>
              <SelectTrigger id="pdf-import-mode" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="text_only">{translate(locale, 'pdfImport.mode.textOnly')}</SelectItem>
                <SelectItem value="ocr_when_needed">{translate(locale, 'pdfImport.mode.ocrWhenNeeded')}</SelectItem>
                <SelectItem value="ocr_all_pages">{translate(locale, 'pdfImport.mode.ocrAllPages')}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-2">
            <label className="text-sm font-medium text-foreground" htmlFor="pdf-import-language">
              {translate(locale, 'pdfImport.language.label')}
            </label>
            <Input
              id="pdf-import-language"
              value={ocrLanguage}
              onChange={(event) => setOcrLanguage(event.target.value)}
              placeholder={translate(locale, 'pdfImport.language.placeholder')}
              disabled={working || ocrMode === 'text_only'}
            />
          </div>
        </div>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose} disabled={working}>
            {translate(locale, 'common.cancel')}
          </Button>
          <Button type="button" onClick={() => onSubmit({ ocrLanguage, ocrMode })} disabled={working}>
            {working ? translate(locale, 'pdfImport.dialog.working') : translate(locale, 'pdfImport.dialog.submit')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
